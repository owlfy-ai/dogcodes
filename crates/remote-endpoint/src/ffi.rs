use crate::{Error, Session};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    ffi::{CString, c_char},
    sync::{Mutex, OnceLock},
};
use zeroize::Zeroize;

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum Call {
    Keypair,
    New {
        initiator: bool,
        secret: String,
        peer: Option<String>,
        psk: Option<String>,
        context: String,
    },
    Write {
        handle: u64,
    },
    Read {
        handle: u64,
        bytes: String,
    },
    Peer {
        handle: u64,
    },
    Seal {
        handle: u64,
        bytes: String,
    },
    Open {
        handle: u64,
        bytes: String,
    },
    Close {
        handle: u64,
    },
}
#[derive(Default)]
struct Registry {
    next: u64,
    sessions: HashMap<u64, Session>,
}
static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
fn key(value: &str) -> Result<[u8; 32], Error> {
    hex::decode(value)
        .map_err(|_| Error::InvalidInput)?
        .try_into()
        .map_err(|_| Error::InvalidInput)
}

fn call(input: &[u8]) -> Result<Value, Error> {
    let command: Call = serde_json::from_slice(input).map_err(|_| Error::InvalidInput)?;
    let mut registry = REGISTRY
        .get_or_init(Mutex::default)
        .lock()
        .map_err(|_| Error::Closed)?;
    match command {
        Call::Keypair => {
            let mut key = crate::keypair()?;
            let result =
                json!({"secret":hex::encode(&key.private), "public":hex::encode(&key.public)});
            key.private.zeroize();
            Ok(result)
        }
        Call::New {
            initiator,
            mut secret,
            peer,
            mut psk,
            context,
        } => {
            let result = (|| {
                if registry.sessions.len() >= 64 {
                    return Err(Error::Capacity);
                }
                let secret = zeroize::Zeroizing::new(key(&secret)?);
                let psk = psk
                    .as_deref()
                    .map(key)
                    .transpose()?
                    .map(zeroize::Zeroizing::new);
                let session = Session::new(
                    initiator,
                    &secret,
                    peer.as_deref().map(key).transpose()?,
                    psk.as_deref(),
                    context.as_bytes(),
                )?;
                registry.next = registry.next.checked_add(1).ok_or(Error::Capacity)?;
                let handle = registry.next;
                registry.sessions.insert(handle, session);
                Ok(json!({"handle":handle}))
            })();
            secret.zeroize();
            if let Some(psk) = &mut psk {
                psk.zeroize();
            }
            result
        }
        Call::Close { handle } => {
            registry.sessions.remove(&handle);
            Ok(json!({}))
        }
        Call::Write { handle } => Ok(
            json!({"bytes":hex::encode(registry.sessions.get_mut(&handle).ok_or(Error::Closed)?.write_handshake()?)}),
        ),
        Call::Read { handle, bytes } => {
            registry
                .sessions
                .get_mut(&handle)
                .ok_or(Error::Closed)?
                .read_handshake(&hex::decode(bytes).map_err(|_| Error::InvalidInput)?)?;
            Ok(json!({}))
        }
        Call::Peer { handle } => {
            let session = registry.sessions.get(&handle).ok_or(Error::Closed)?;
            Ok(
                json!({"public":hex::encode(session.authenticated_peer()?), "binding":hex::encode(session.binding()?)}),
            )
        }
        Call::Seal { handle, mut bytes } => {
            let input = hex::decode(&bytes).map_err(|_| Error::InvalidInput);
            bytes.zeroize();
            let input = zeroize::Zeroizing::new(input?);
            Ok(
                json!({"bytes":hex::encode(registry.sessions.get_mut(&handle).ok_or(Error::Closed)?.seal(&input)?)}),
            )
        }
        Call::Open { handle, bytes } => {
            let mut plain = registry
                .sessions
                .get_mut(&handle)
                .ok_or(Error::Closed)?
                .open(&hex::decode(bytes).map_err(|_| Error::InvalidInput)?)?;
            let result = json!({"bytes":hex::encode(&plain)});
            plain.zeroize();
            Ok(result)
        }
    }
}

/// JSON request/response boundary for Dart. Never prints inputs or keys.
/// # Safety
/// `input` must point to `len` readable bytes for the duration of this call.
/// Free the returned pointer exactly once with `dogcodes_endpoint_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dogcodes_endpoint_call(input: *const u8, len: usize) -> *mut c_char {
    let result = std::panic::catch_unwind(|| {
        if input.is_null() || len > 196608 {
            return Err(Error::InvalidInput);
        }
        // SAFETY: the caller owns a buffer of len bytes for this synchronous call.
        let bytes = unsafe { std::slice::from_raw_parts(input, len) };
        call(bytes)
    })
    .unwrap_or(Err(Error::Closed));
    let value = match result {
        Ok(value) => json!({"ok":true,"value":value}),
        Err(error) => json!({"ok":false,"error":match error {
            Error::InvalidInput => "invalid_input", Error::Authentication => "authentication_failed",
            Error::WrongPhase => "wrong_phase", Error::Closed => "closed", Error::Capacity => "capacity",
        }}),
    };
    match CString::new(value.to_string()) {
        Ok(value) => value.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}
/// # Safety
/// `pointer` is null or a live pointer returned by `dogcodes_endpoint_call`, never previously freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dogcodes_endpoint_free(pointer: *mut c_char) {
    if !pointer.is_null() {
        // SAFETY: caller returns the allocation from this library exactly once.
        let value = unsafe { CString::from_raw(pointer) };
        value.into_bytes_with_nul().zeroize();
    }
}
