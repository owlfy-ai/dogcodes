//! Runtime authority follows an admission, even when the same native thread is reused.
use super::{DriverError, SessionCommand, SessionHandle, lifecycle, mpsc};
use std::sync::{Arc, Mutex};

pub(super) type Scope = Arc<Mutex<SessionHandle>>;

#[derive(Clone)]
pub(super) struct ResidentSession {
    pub commands: mpsc::Sender<ScopedCommand>,
    pub native: Option<Arc<lifecycle::NativeProcess>>,
    pub scope: Scope,
}

impl ResidentSession {
    pub fn handle(&self) -> SessionHandle {
        self.scope.lock().expect("Codex session scope").clone()
    }

    pub fn sender(&self) -> SessionSender {
        self.sender_for(self.handle())
    }

    pub fn sender_for(&self, handle: SessionHandle) -> SessionSender {
        SessionSender {
            commands: self.commands.clone(),
            handle,
        }
    }

    pub fn is_closed(&self) -> bool {
        self.commands.is_closed()
    }
}

pub(super) struct ScopedCommand {
    pub handle: SessionHandle,
    pub command: SessionCommand,
}

#[derive(Clone)]
pub(super) struct SessionSender {
    commands: mpsc::Sender<ScopedCommand>,
    handle: SessionHandle,
}

impl SessionSender {
    pub async fn send(
        &self,
        command: SessionCommand,
    ) -> Result<(), mpsc::error::SendError<ScopedCommand>> {
        self.commands
            .send(ScopedCommand {
                handle: self.handle.clone(),
                command,
            })
            .await
    }

    #[cfg(all(test, unix))]
    pub fn same_channel(&self, other: &Self) -> bool {
        self.commands.same_channel(&other.commands)
    }
}

pub(super) fn stale_scope() -> DriverError {
    DriverError::SessionIncompatible("codex session handle is no longer current".into())
}

// Check again at consumption, not just map lookup: a queued cancel/approval can
// outlive idle reuse of the same actor and must never bind to its next turn.
pub(super) async fn next_command(
    commands: &mut mpsc::Receiver<ScopedCommand>,
    scope: &Scope,
) -> Option<SessionCommand> {
    while let Some(ScopedCommand { handle, command }) = commands.recv().await {
        if scope
            .lock()
            .expect("Codex session scope")
            .same_scope(&handle)
        {
            return Some(command);
        }
        match command {
            SessionCommand::Prompt { response, .. } => {
                let _ = response.send(Err(stale_scope()));
            }
            SessionCommand::CheckReady { response, .. }
            | SessionCommand::Answer { response, .. }
            | SessionCommand::Configure { response, .. }
            | SessionCommand::Interrupt { response }
            | SessionCommand::Approve { response, .. }
            | SessionCommand::Shutdown { response } => {
                let _ = response.send(Err(stale_scope()));
            }
        }
    }
    None
}
