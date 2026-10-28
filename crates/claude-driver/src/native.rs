//! A session's OS process group/job outlives protocol observers until proven closed.
use agent_runtime::OwnedChild;
use driver_api::DriverError;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{io::DuplexStream, process::ChildStdin, sync::watch};

pub(super) struct NativeProcess {
    stopping: AtomicBool,
    close: watch::Sender<bool>,
    exited: watch::Receiver<bool>,
}

impl NativeProcess {
    pub fn start(child: OwnedChild) -> Result<(Arc<Self>, ChildStdin, DuplexStream), DriverError> {
        Self::start_with_grace(child, Duration::from_millis(300))
    }
    /// Readiness closes stdin after initialize and preserves the CLI's existing
    /// five-second auth-persistence window, including observer cancellation.
    pub fn start_with_grace(
        mut child: OwnedChild,
        grace: Duration,
    ) -> Result<(Arc<Self>, ChildStdin, DuplexStream), DriverError> {
        let stdin = child.stdin.take().ok_or_else(unavailable)?;
        let mut stdout = child.stdout.take().ok_or_else(unavailable)?;
        let mut stderr = child.stderr.take().ok_or_else(unavailable)?;
        let (reader, mut writer) = tokio::io::duplex(64 * 1024);
        let output = tokio::spawn(async move {
            let _ = tokio::io::copy(&mut stdout, &mut writer).await;
        });
        let diagnostics = tokio::spawn(async move {
            let _ = tokio::io::copy(&mut stderr, &mut tokio::io::sink()).await;
        });
        let (close, mut closing) = watch::channel(false);
        let (completed, exited) = watch::channel(false);
        let native = Arc::new(Self {
            stopping: AtomicBool::new(false),
            close,
            exited,
        });
        let control = native.clone();
        tokio::spawn(async move {
            tokio::select! {
                () = async {
                    while !*closing.borrow_and_update() {
                        if closing.changed().await.is_err() { break; }
                    }
                } => {},
                () = async {
                    while !output.is_finished() {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                } => {},
                _ = child.wait() => {},
            }
            control.stopping.store(true, Ordering::Release);
            if !matches!(tokio::time::timeout(grace, child.wait()).await, Ok(Ok(_))) {
                let mut delay = 1;
                // Continue the same owned termination; no CLI control replay or
                // PID lookup, including when an observer has stopped waiting.
                while child.terminate().await.is_err() {
                    tokio::time::sleep(Duration::from_secs(delay)).await;
                    delay = (delay * 2).min(30);
                }
            }
            let mut output = output;
            if tokio::time::timeout(Duration::from_secs(1), &mut output)
                .await
                .is_err()
            {
                output.abort();
                let _ = output.await;
            }
            diagnostics.abort();
            let _ = diagnostics.await;
            completed.send_replace(true);
        });
        Ok((native, stdin, reader))
    }

    pub fn stopping(&self) -> bool {
        self.stopping.load(Ordering::Acquire)
    }

    pub fn request_close(&self) {
        self.stopping.store(true, Ordering::Release);
        self.close.send_replace(true);
    }

    pub async fn confirmed(&self) {
        let mut exited = self.exited.clone();
        loop {
            if *exited.borrow_and_update() {
                return;
            }
            if exited.changed().await.is_err() {
                std::future::pending::<()>().await;
            }
        }
    }

    pub fn observe_owner(self: &Arc<Self>, mut owner: watch::Receiver<bool>) {
        let native = self.clone();
        tokio::spawn(async move {
            tokio::select! {
                () = async {
                    while !*owner.borrow_and_update() {
                        if owner.changed().await.is_err() { break; }
                    }
                } => native.request_close(),
                () = native.confirmed() => {},
            }
        });
    }
}

fn unavailable() -> DriverError {
    DriverError::Disconnected("Claude process pipes unavailable".into())
}

#[cfg(test)]
mod proof_tests {
    use super::*;

    #[tokio::test]
    async fn passive_observation_neither_closes_the_process_nor_finishes_early() {
        let (close, _closing) = watch::channel(false);
        let (completed, exited) = watch::channel(false);
        let native = NativeProcess {
            stopping: AtomicBool::new(false),
            close,
            exited,
        };
        let mut proof = std::pin::pin!(native.confirmed());
        assert!(
            tokio::time::timeout(Duration::from_millis(20), &mut proof)
                .await
                .is_err()
        );
        assert!(!native.stopping());
        assert!(!*native.close.borrow());
        completed.send_replace(true);
        tokio::time::timeout(Duration::from_secs(1), proof)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn supervisor_loss_is_not_a_positive_exit_proof() {
        let (close, _closing) = watch::channel(false);
        let (completed, exited) = watch::channel(false);
        let native = NativeProcess {
            stopping: AtomicBool::new(false),
            close,
            exited,
        };
        drop(completed);
        assert!(
            tokio::time::timeout(Duration::from_millis(20), native.confirmed())
                .await
                .is_err()
        );
        assert!(!native.stopping());
    }
}
