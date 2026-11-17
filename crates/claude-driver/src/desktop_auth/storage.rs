//! Electron safeStorage formats; fail closed for unknown encryption versions.
//! OS helpers have fixed arguments, bounded pipes/time, and no retained stderr.
use agent_runtime::{Pipe, SharedTarget, TargetCommand};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    sync::LazyLock,
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::Mutex,
};
use zeroize::Zeroizing;

const LIMIT: u64 = 2 * 1024 * 1024;
type CachedKey = (PathBuf, Instant, Option<Zeroizing<Vec<u8>>>);
static KEY: LazyLock<Mutex<Option<CachedKey>>> = LazyLock::new(|| Mutex::new(None));

pub(super) fn config_paths(target: &SharedTarget) -> Vec<PathBuf> {
    let home = target
        .inherited("HOME")
        .or_else(|| target.inherited("USERPROFILE"))
        .map(PathBuf::from);
    let Some(home) = home.filter(|p| p.is_absolute()) else {
        return vec![];
    };
    // Unit/protocol fixtures must never consult the real user's desktop login.
    #[cfg(test)]
    if std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .as_ref()
        == Some(&home)
    {
        return vec![];
    }
    let mut roots = Vec::new();
    if cfg!(target_os = "macos") {
        roots.push(home.join("Library/Application Support/Claude"));
    } else if cfg!(windows) {
        roots.push(
            target
                .inherited("APPDATA")
                .map_or_else(|| home.join("AppData/Roaming"), PathBuf::from)
                .join("Claude"),
        );
        let local = target
            .inherited("LOCALAPPDATA")
            .map_or_else(|| home.join("AppData/Local"), PathBuf::from);
        if let Ok(packages) = std::fs::read_dir(local.join("Packages")) {
            for entry in packages.filter_map(Result::ok).take(256) {
                if entry
                    .file_name()
                    .to_str()
                    .is_some_and(|s| s.starts_with("Claude_"))
                {
                    roots.push(entry.path().join("LocalCache/Roaming/Claude"));
                }
            }
        }
    } else if cfg!(target_os = "linux") {
        roots.push(
            target
                .inherited("XDG_CONFIG_HOME")
                .map_or_else(|| home.join(".config"), PathBuf::from)
                .join("Claude"),
        );
    }
    roots
        .into_iter()
        .filter(|root| root.is_absolute() && root.join("claude-code").is_dir())
        .map(|root| root.join("config.json"))
        .collect()
}

pub(super) async fn read_json(path: &Path) -> Option<Value> {
    let mut bytes = Zeroizing::new(Vec::new());
    let file = tokio::fs::File::open(path).await.ok()?;
    if !file.metadata().await.ok()?.is_file() {
        return None;
    }
    file.take(LIMIT + 1).read_to_end(&mut bytes).await.ok()?;
    if bytes.len() as u64 > LIMIT {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}

pub(super) async fn decrypt(file: &Path, encoded: &str) -> Option<Zeroizing<Vec<u8>>> {
    if encoded.len() as u64 > LIMIT {
        return None;
    }
    let encrypted = STANDARD.decode(encoded).ok()?;
    let prefix = if cfg!(target_os = "linux") {
        b"v11"
    } else {
        b"v10"
    };
    if !encrypted.starts_with(prefix) {
        return None;
    }
    let root = file.parent()?;
    let mut cache = KEY.lock().await;
    let hit = cache.as_ref().filter(|(path, at, key)| {
        path == root && at.elapsed() < Duration::from_secs(if key.is_some() { 300 } else { 30 })
    });
    let key = if let Some((_, _, key)) = hit {
        key.clone()?
    } else {
        let key = platform_key(root).await;
        *cache = Some((root.into(), Instant::now(), key.clone()));
        key?
    };
    if cfg!(windows) {
        decrypt_gcm(&key, &encrypted)
    } else {
        decrypt_cbc(
            &key,
            &encrypted,
            if cfg!(target_os = "macos") {
                b"v10"
            } else {
                b"v11"
            },
        )
    }
}

fn decrypt_cbc(key: &[u8], data: &[u8], prefix: &[u8]) -> Option<Zeroizing<Vec<u8>>> {
    use cbc::cipher::{BlockDecryptMut, KeyIvInit, block_padding::Pkcs7};
    let mut body = Zeroizing::new(data.strip_prefix(prefix)?.to_vec());
    let decrypted = cbc::Decryptor::<aes::Aes128>::new_from_slices(key, &[b' '; 16])
        .ok()?
        .decrypt_padded_mut::<Pkcs7>(&mut body)
        .ok()?;
    Some(Zeroizing::new(decrypted.to_vec()))
}

fn decrypt_gcm(key: &[u8], data: &[u8]) -> Option<Zeroizing<Vec<u8>>> {
    use aes_gcm::{Aes256Gcm, KeyInit, Nonce, aead::Aead};
    let body = data.strip_prefix(b"v10")?;
    if body.len() < 12 + 16 {
        return None;
    }
    let cipher = Aes256Gcm::new_from_slice(key).ok()?;
    Some(Zeroizing::new(
        cipher
            .decrypt(Nonce::from_slice(&body[..12]), &body[12..])
            .ok()?,
    ))
}

#[cfg(any(target_os = "macos", target_os = "linux", test))]
fn derive(password: &[u8], rounds: u32) -> Zeroizing<Vec<u8>> {
    let mut key = Zeroizing::new(vec![0; 16]);
    pbkdf2::pbkdf2_hmac::<sha1::Sha1>(password, b"saltysalt", rounds, &mut key);
    key
}

#[cfg(all(test, unix))]
pub(super) async fn fixture(file: &Path, account: &str, entries: &Value) {
    use cbc::cipher::{BlockEncryptMut, KeyIvInit, block_padding::Pkcs7};
    let key = derive(b"private-offline-fixture", 1003);
    let text = entries.to_string();
    let mut body = vec![0; text.len() + 16];
    body[..text.len()].copy_from_slice(text.as_bytes());
    let encrypted = if cfg!(windows) {
        use aes_gcm::{Aes256Gcm, KeyInit, Nonce, aead::Aead};
        let key = Zeroizing::new(vec![42; 32]);
        let encrypted = Aes256Gcm::new_from_slice(&key)
            .unwrap()
            .encrypt(Nonce::from_slice(&[1; 12]), text.as_bytes())
            .unwrap();
        *KEY.lock().await = Some((file.parent().unwrap().into(), Instant::now(), Some(key)));
        [b"v10".as_slice(), &[1; 12], &encrypted].concat()
    } else {
        let encrypted = cbc::Encryptor::<aes::Aes128>::new_from_slices(&key, &[b' '; 16])
            .unwrap()
            .encrypt_padded_mut::<Pkcs7>(&mut body, text.len())
            .unwrap();
        *KEY.lock().await = Some((file.parent().unwrap().into(), Instant::now(), Some(key)));
        [
            if cfg!(target_os = "macos") {
                b"v10"
            } else {
                b"v11"
            }
            .as_slice(),
            encrypted,
        ]
        .concat()
    };
    tokio::fs::create_dir_all(file.parent().unwrap().join("claude-code"))
        .await
        .unwrap();
    tokio::fs::write(file, serde_json::to_vec(&serde_json::json!({"lastKnownAccountUuid":account,"oauth:tokenCacheV2":STANDARD.encode(encrypted)})).unwrap()).await.unwrap();
}

#[cfg(target_os = "macos")]
async fn platform_key(_root: &Path) -> Option<Zeroizing<Vec<u8>>> {
    // macOS may ask the user to allow access to Claude's Safe Storage item.
    // Denial/timeout falls back to the CLI, without changing the item's ACL.
    let mut password = helper(
        "/usr/bin/security",
        &[
            "find-generic-password",
            "-s",
            "Claude Safe Storage",
            "-a",
            "Claude Key",
            "-w",
        ],
        None,
    )
    .await?;
    if password.last() == Some(&b'\n') {
        password.pop();
    }
    if password.is_empty() {
        return None;
    }
    Some(derive(&password, 1003))
}

#[cfg(windows)]
async fn platform_key(root: &Path) -> Option<Zeroizing<Vec<u8>>> {
    let state = read_json(&root.join("Local State")).await?;
    let encoded = state["os_crypt"]["encrypted_key"].as_str()?;
    let protected = STANDARD.decode(encoded).ok()?;
    let protected = protected.strip_prefix(b"DPAPI")?;
    let input = STANDARD.encode(protected) + "\n";
    // Use the current Windows user's DPAPI, never another user's store. No
    // secrets appear in process arguments or a temporary PowerShell script.
    let script = "$ErrorActionPreference='Stop'; Add-Type -AssemblyName System.Security; try { $b=[Convert]::FromBase64String([Console]::In.ReadLine()); $k=[Security.Cryptography.ProtectedData]::Unprotect($b,$null,[Security.Cryptography.DataProtectionScope]::CurrentUser); [Console]::Out.Write([Convert]::ToBase64String($k)) } catch { exit 1 }";
    let program = std::env::var_os("SystemRoot")
        .map(PathBuf::from)?
        .join("System32/WindowsPowerShell/v1.0/powershell.exe");
    let text = helper(
        program,
        &["-NoProfile", "-NonInteractive", "-Command", script],
        Some(input.as_bytes()),
    )
    .await?;
    let key = Zeroizing::new(STANDARD.decode(&text).ok()?);
    (key.len() == 32).then_some(key)
}

#[cfg(target_os = "linux")]
async fn platform_key(_root: &Path) -> Option<Zeroizing<Vec<u8>>> {
    // GNOME Secret Service. No plaintext/basic_text fallback or guessed key.
    let mut password = helper("secret-tool", &["lookup", "application", "Claude"], None).await?;
    if password.last() == Some(&b'\n') {
        password.pop();
    }
    if password.is_empty() {
        return None;
    }
    Some(derive(&password, 1))
}

#[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
async fn platform_key(_root: &Path) -> Option<Zeroizing<Vec<u8>>> {
    None
}

async fn helper(
    program: impl AsRef<std::ffi::OsStr>,
    args: &[&str],
    input: Option<&[u8]>,
) -> Option<Zeroizing<Vec<u8>>> {
    let mut command = TargetCommand::new(agent_runtime::local_target(), program);
    command
        .args(args)
        .stdio(Pipe::Piped, Pipe::Piped, Pipe::Null);
    for key in [
        "HOME",
        "USER",
        "LOGNAME",
        "PATH",
        "DBUS_SESSION_BUS_ADDRESS",
        "XDG_RUNTIME_DIR",
    ] {
        command.inherit(key);
    }
    let mut child = command.spawn().ok()?;
    let mut stdin = child.stdin.take()?;
    let stdout = child.stdout.take()?;
    let mut bytes = Zeroizing::new(Vec::new());
    let result = tokio::time::timeout(Duration::from_secs(8), async {
        if let Some(input) = input {
            stdin.write_all(input).await?;
        }
        drop(stdin);
        stdout.take(65_537).read_to_end(&mut bytes).await?;
        child.wait().await
    })
    .await;
    let success = matches!(result, Ok(Ok(status)) if status.success());
    let _ = child.terminate().await;
    (success && !bytes.is_empty() && bytes.len() <= 65_536).then_some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn electron_cbc_and_gcm_formats_reject_wrong_keys_and_unknown_versions() {
        use aes_gcm::{Aes256Gcm, KeyInit, Nonce, aead::Aead};
        use cbc::cipher::{BlockEncryptMut, KeyIvInit, block_padding::Pkcs7};
        let key = derive(b"private-fixture", 1003);
        let mut block = [0u8; 32];
        block[..7].copy_from_slice(b"fixture");
        let body = cbc::Encryptor::<aes::Aes128>::new_from_slices(&key, &[b' '; 16])
            .unwrap()
            .encrypt_padded_mut::<Pkcs7>(&mut block, 7)
            .unwrap();
        let data = [b"v10".as_slice(), body].concat();
        assert_eq!(&**decrypt_cbc(&key, &data, b"v10").unwrap(), b"fixture");
        assert!(decrypt_cbc(&key, &data, b"v11").is_none());
        assert!(decrypt_cbc(&[0; 16], &data, b"v10").is_none());
        let cipher = Aes256Gcm::new_from_slice(&[42; 32]).unwrap();
        let encrypted = cipher
            .encrypt(Nonce::from_slice(&[1; 12]), b"fixture".as_ref())
            .unwrap();
        let data = [b"v10".as_slice(), &[1; 12], &encrypted].concat();
        assert_eq!(&**decrypt_gcm(&[42; 32], &data).unwrap(), b"fixture");
        assert!(decrypt_gcm(&[0; 32], &data).is_none());
        assert!(decrypt_gcm(&[42; 32], b"v20unknown").is_none());
    }
}
