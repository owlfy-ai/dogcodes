//! Opt-in process metadata. Never log environment values, stdin or CLI output.
use std::{
    process::ExitStatus,
    sync::{
        OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};
use tokio::process::Command;

static RECORDER: OnceLock<fn(&str, &str)> = OnceLock::new();
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

pub fn install_process_diagnostics(recorder: fn(&str, &str)) {
    let _ = RECORDER.set(recorder);
}

/// Records only recognized command words. Prompts, shell source, tokens and
/// arbitrary positional arguments remain hidden even before file redaction.
fn command_words(command: &Command) -> String {
    command
        .as_std()
        .get_args()
        .take(8)
        .map(|arg| match arg.to_str().unwrap_or_default() {
            word @ ("--version"
            | "-v"
            | "--help"
            | "--acp"
            | "acp"
            | "agent"
            | "stdio"
            | "app-server"
            | "--stdio"
            | "--disable-auto-update"
            | "--experimental-acp"
            | "--output-format"
            | "stream-json"
            | "--input-format"
            | "--print"
            | "--rpc"
            | "rpc"
            | "--mode"
            | "models"
            | "update"
            | "--check") => word,
            _ => "<omitted>",
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub struct ProcessDiagnostic {
    id: u64,
    started: Instant,
    child_pid: Option<u32>,
    finished: bool,
}

impl ProcessDiagnostic {
    pub fn begin(command: &Command, strategy: &str) -> Self {
        let trace = Self {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            started: Instant::now(),
            child_pid: None,
            finished: false,
        };
        trace.record(
            "process_spawn_attempt",
            &format!(
                "strategy={strategy} program={} cwd={} argc={} command_words={:?}",
                command.as_std().get_program().to_string_lossy(),
                command
                    .as_std()
                    .get_current_dir()
                    .map_or_else(|| "<inherited>".into(), |path| path.to_string_lossy()),
                command.as_std().get_args().count(),
                command_words(command),
            ),
        );
        trace
    }

    pub fn spawned(&mut self, pid: Option<u32>) {
        self.child_pid = pid;
        self.record("process_spawned", "");
    }

    pub fn failed(&mut self, stage: &str, error: &std::io::Error) {
        self.finished = true;
        self.record(
            "process_failed",
            &format!(
                "stage={stage} kind={:?} os_error={:?}",
                error.kind(),
                error.raw_os_error()
            ),
        );
    }

    pub fn exited(&mut self, status: ExitStatus) {
        if !self.finished {
            self.finished = true;
            self.record(
                "process_exited",
                &format!("success={} exit_code={:?}", status.success(), status.code()),
            );
        }
    }

    pub fn record(&self, event: &str, detail: &str) {
        if let Some(recorder) = RECORDER.get() {
            recorder(
                event,
                &format!(
                    "launch_id={} child_pid={:?} elapsed_ms={} {detail}",
                    self.id,
                    self.child_pid,
                    self.started.elapsed().as_millis()
                ),
            );
        }
    }
}

impl Drop for ProcessDiagnostic {
    fn drop(&mut self) {
        if !self.finished {
            self.record("process_observer_dropped", "exit_unconfirmed=true");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_metadata_omits_payloads_and_credentials() {
        let mut command = Command::new("agent.exe");
        command.args([
            "agent",
            "stdio",
            "--token=secret",
            "private prompt",
            "-c",
            "echo secret",
        ]);
        command.env("API_KEY", "secret environment");
        let words = command_words(&command);
        assert_eq!(words, "agent stdio <omitted> <omitted> <omitted> <omitted>");
    }
}
