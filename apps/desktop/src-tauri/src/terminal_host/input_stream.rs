//! Ephemeral input sequencing. Never persist keystrokes or replay a write.
use super::{Session, TerminalHost, TerminalStatus};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

pub const LIMIT: usize = 16 * 1024;
#[derive(Default)]
pub(super) struct State {
    revision: u64,
    lease: Option<Lease>,
    uncertain: bool,
    draining: bool,
}
struct Lease {
    id: String,
    owner: String,
    valid: Arc<AtomicBool>,
    sequence: u64,
    digest: Option<String>,
    phase: &'static str,
    written: usize,
    acknowledge: bool,
}
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
enum Call {
    Inspect {
        #[serde(rename = "terminalId")]
        terminal: String,
        #[serde(rename = "expectedEpoch")]
        epoch: String,
    },
    Open {
        #[serde(rename = "terminalId")]
        terminal: String,
        #[serde(rename = "expectedEpoch")]
        epoch: String,
        #[serde(rename = "expectedRevision")]
        revision: String,
        #[serde(rename = "streamId")]
        stream: String,
        #[serde(rename = "acknowledgeUncertain", default)]
        acknowledge: bool,
    },
    Write {
        #[serde(rename = "terminalId")]
        terminal: String,
        #[serde(rename = "expectedEpoch")]
        epoch: String,
        #[serde(rename = "streamId")]
        stream: String,
        sequence: String,
        revision: String,
        data: String,
    },
}
fn number(value: &str) -> Result<u64, String> {
    if value.is_empty() || value.len() > 20 || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err("backendText.mf53158f0".into());
    }
    value.parse().map_err(|_| "backendText.m4ff3c023".into())
}
impl State {
    fn snapshot(&self, session: &Session, owner: &str) -> Result<Value, String> {
        let info = session.info()?;
        Ok(
            json!({"terminalId":info.id,"epoch":info.epoch,"revision":self.revision.to_string(),
            "streamId":self.lease.as_ref().map(|l|&l.id),"owned":self.lease.as_ref().is_some_and(|l|l.owner==owner),
            "sequence":self.lease.as_ref().map_or(0,|l|l.sequence).to_string(),
            "state":self.lease.as_ref().map_or("idle",|l|l.phase),"uncertain":self.uncertain,
            "digest":self.lease.as_ref().and_then(|l|l.digest.as_ref()),"written":self.lease.as_ref().map_or(0,|l|l.written),
            "status":info.status}),
        )
    }
}
impl TerminalHost {
    pub async fn input_operation(&self, owner: &str, args: Value) -> Result<Value, String> {
        let call: Call = serde_json::from_value(args).map_err(|_| "backendText.m6f71fd02")?;
        let (id, epoch) = match &call {
            Call::Inspect { terminal, epoch }
            | Call::Open {
                terminal, epoch, ..
            }
            | Call::Write {
                terminal, epoch, ..
            } => (terminal, epoch),
        };
        let session = self.session(id)?;
        let info = session.info()?;
        if info.epoch != *epoch || self.epoch != *epoch {
            return Err("backendText.m64305afe".into());
        }
        match call {
            Call::Inspect { .. } => session
                .input_stream
                .lock()
                .map_err(|_| "backendText.m906123c7")?
                .snapshot(&session, owner),
            Call::Open {
                revision,
                stream,
                acknowledge,
                ..
            } => {
                uuid::Uuid::parse_str(&stream).map_err(|_| "backendText.ma88bf82f")?;
                let revision = number(&revision)?;
                let start_drain = {
                    let mut state = session
                        .input_stream
                        .lock()
                        .map_err(|_| "backendText.m906123c7")?;
                    if let Some(lease) = &state.lease
                        && lease.id == stream
                        && lease.owner == owner
                        && state.revision
                            == revision.checked_add(1).ok_or("backendText.m02f04e3c")?
                    {
                        return state.snapshot(&session, owner);
                    }
                    if state.revision != revision {
                        return Err("backendText.m5a475c85".into());
                    }
                    if info.status != TerminalStatus::Running {
                        return Err("backendText.m5f03aa0a".into());
                    }
                    if let Some(old) = &state.lease {
                        old.valid.store(false, Ordering::Release);
                        if old.phase == "writing" {
                            state.uncertain = true;
                        }
                    }
                    state.revision = state
                        .revision
                        .checked_add(1)
                        .ok_or("backendText.m02f04e3c")?;
                    state.lease = Some(Lease {
                        id: stream.clone(),
                        owner: owner.into(),
                        valid: Arc::new(AtomicBool::new(false)),
                        sequence: 0,
                        digest: None,
                        phase: "opening",
                        written: 0,
                        acknowledge,
                    });
                    let start = !state.draining;
                    state.draining = true;
                    start
                };
                if start_drain {
                    let opening = session.clone();
                    let drain = tokio::task::spawn_blocking(move || -> Result<(), String> {
                        // One drain worker per terminal. New CAS reservations
                        // replace the pending target without adding blocked
                        // threads if a Windows writer is still in the OS.
                        let input = opening.input.lock().map_err(|_| "backendText.m82e48394")?;
                        let mut state = opening
                            .input_stream
                            .lock()
                            .map_err(|_| "backendText.m906123c7")?;
                        state.draining = false;
                        if opening.info()?.status != TerminalStatus::Running || input.is_none() {
                            return Err("backendText.m72d2b1d2".into());
                        }
                        let acknowledge = state
                            .lease
                            .as_ref()
                            .ok_or("backendText.m0f7fab99")?
                            .acknowledge;
                        let blocked = state.uncertain && !acknowledge;
                        if acknowledge {
                            state.uncertain = false;
                        }
                        let lease = state.lease.as_mut().ok_or("backendText.m0f7fab99")?;
                        lease.phase = if blocked { "blocked" } else { "ready" };
                        lease.valid.store(!blocked, Ordering::Release);
                        Ok(())
                    });
                    if let Ok(result) = tokio::time::timeout(Duration::from_secs(2), drain).await {
                        result.map_err(|_| "backendText.m0ab260b2")??;
                    }
                }
                session
                    .input_stream
                    .lock()
                    .map_err(|_| "backendText.m906123c7")?
                    .snapshot(&session, owner)
            }
            Call::Write {
                stream,
                sequence,
                revision,
                data,
                ..
            } => {
                if data.len() > LIMIT.div_ceil(3) * 4 {
                    return Err("backendText.m687c7909".into());
                }
                let bytes = STANDARD.decode(data).map_err(|_| "backendText.m4828b4c2")?;
                if bytes.is_empty() || bytes.len() > LIMIT {
                    return Err("backendText.m556f2d92".into());
                }
                let revision = number(&revision)?;
                let sequence = number(&sequence)?;
                let digest = format!("{:x}", Sha256::digest(&bytes));
                let valid = {
                    let mut state = session
                        .input_stream
                        .lock()
                        .map_err(|_| "backendText.m906123c7")?;
                    if state.revision != revision {
                        return Err("backendText.md272b6e1".into());
                    }
                    let uncertain = state.uncertain;
                    let lease = state
                        .lease
                        .as_mut()
                        .filter(|l| l.id == stream && l.owner == owner)
                        .ok_or("backendText.m4efc40f3")?;
                    if sequence > 0 && sequence == lease.sequence {
                        if lease.digest.as_deref() != Some(&digest) {
                            return Err("backendText.m19283dc4".into());
                        }
                        return state.snapshot(&session, owner);
                    }
                    if uncertain
                        || !lease.valid.load(Ordering::Acquire)
                        || !matches!(lease.phase, "ready" | "applied")
                        || sequence
                            != lease
                                .sequence
                                .checked_add(1)
                                .ok_or("backendText.mb9165b0e")?
                    {
                        return Err("backendText.ma270fd9e".into());
                    }
                    lease.sequence = sequence;
                    lease.digest = Some(digest);
                    lease.phase = "writing";
                    lease.written = 0;
                    lease.valid.clone()
                };
                let owner = owner.to_owned();
                let writing = session.clone();
                tokio::task::spawn_blocking(move || {
                    let mut input = writing.input.lock().map_err(|_| "backendText.m82e48394")?;
                    let start = Instant::now();
                    let mut written = 0;
                    let result = (|| -> Result<(), String> {
                        let writer = input.as_mut().ok_or("backendText.m892b1bec")?;
                        while written < bytes.len() {
                            if !valid.load(Ordering::Acquire)
                                || writing.info()?.status != TerminalStatus::Running
                            {
                                return Err("backendText.mf52aff83".into());
                            }
                            if start.elapsed() > Duration::from_secs(5) {
                                return Err("backendText.m4a572aae".into());
                            }
                            match writer.write(&bytes[written..]) {
                                Ok(0) => return Err("backendText.mde02200f".into()),
                                Ok(count) => written += count,
                                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                                    std::thread::sleep(Duration::from_millis(5));
                                }
                                Err(_) => return Err("backendText.m7f039774".into()),
                            }
                        }
                        writer.flush().map_err(|_| "backendText.maa38c28c".into())
                    })();
                    let mut state = writing
                        .input_stream
                        .lock()
                        .map_err(|_| "backendText.m906123c7")?;
                    if result.is_err() && written > 0 {
                        state.uncertain = true;
                    }
                    let lease = state
                        .lease
                        .as_mut()
                        .filter(|l| l.id == stream && l.owner == owner)
                        .ok_or("backendText.mf7b4a6f1")?;
                    lease.written = written;
                    lease.phase = if result.is_ok() {
                        "applied"
                    } else if written > 0 {
                        "uncertain"
                    } else {
                        "rejected"
                    };
                    if result.is_err() {
                        lease.valid.store(false, Ordering::Release);
                    }
                    state.snapshot(&writing, &owner)
                })
                .await
                .map_err(|_| "backendText.m93c965c6")?
            }
        }
    }
}

#[cfg(test)]
#[path = "input_tests.rs"]
mod tests;
