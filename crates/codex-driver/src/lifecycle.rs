//! A private app-server scope, supervised independently of callers and actors.
use super::{DriverError, drain_stderr};
use agent_runtime::OwnedChild;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{io::DuplexStream, process::ChildStdin, sync::watch};

type Outcome = Option<Result<(), DriverError>>;
pub(super) struct NativeProcess {
    stopping: AtomicBool,
    close: watch::Sender<bool>,
    exit: watch::Receiver<Outcome>,
}
impl NativeProcess {
    pub fn start(
        mut child: OwnedChild,
    ) -> Result<(Arc<Self>, DuplexStream, ChildStdin), DriverError> {
        let input = child
            .stdin
            .take()
            .ok_or_else(|| failure("stdin unavailable"))?;
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| failure("stdout unavailable"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| failure("stderr unavailable"))?;
        // Bound unread native output without making its lifetime depend on a
        // partially cancelled protocol read. The transport owns the read end.
        let (output, mut pump_input) = tokio::io::duplex(64 * 1024);
        let pump = tokio::spawn(async move {
            let _ = tokio::io::copy(&mut stdout, &mut pump_input).await;
        });
        let stderr = tokio::spawn(drain_stderr(stderr));
        let (close, mut closing) = watch::channel(false);
        let (completed, exit) = watch::channel(None);
        let native = Arc::new(Self {
            stopping: AtomicBool::new(false),
            close,
            exit,
        });
        let control = native.clone();
        tokio::spawn(async move {
            tokio::select! {
                ()=async { while !*closing.borrow_and_update() { if closing.changed().await.is_err() { break; } } }=>{},
                ()=async { while !pump.is_finished() { tokio::time::sleep(Duration::from_millis(10)).await; } }=>{},
                _=child.wait()=>{},
            }
            control.stopping.store(true, Ordering::Release);
            if !matches!(
                tokio::time::timeout(Duration::from_secs(2), child.wait()).await,
                Ok(Ok(_))
            ) {
                let mut delay = 1;
                while child.terminate().await.is_err() {
                    completed.send_replace(Some(Err(failure("process exit remains unconfirmed"))));
                    tokio::time::sleep(Duration::from_secs(delay)).await;
                    delay = (delay * 2).min(30);
                }
            }
            let mut pump = pump;
            if tokio::time::timeout(Duration::from_secs(1), &mut pump)
                .await
                .is_err()
            {
                pump.abort();
                let _ = pump.await;
            }
            stderr.abort();
            let _ = stderr.await;
            completed.send_replace(Some(Ok(())));
        });
        Ok((native, output, input))
    }
    pub fn stopping(&self) -> bool {
        self.stopping.load(Ordering::Acquire)
    }
    pub fn request_close(&self) {
        self.stopping.store(true, Ordering::Release);
        self.close.send_replace(true);
    }
    pub async fn closed(&self) -> Result<(), DriverError> {
        let mut exit = self.exit.clone();
        tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                if let Some(result) = exit.borrow_and_update().clone() {
                    return result;
                }
                exit.changed()
                    .await
                    .map_err(|_| failure("cleanup ended without confirmation"))?;
            }
        })
        .await
        .map_err(|_| failure("shutdown is still pending confirmation"))?
    }
    pub fn observe_owner(self: &Arc<Self>, mut owner: watch::Receiver<bool>) {
        let native = self.clone();
        tokio::spawn(async move {
            let mut exit = native.exit.clone();
            loop {
                if *owner.borrow() {
                    native.request_close();
                    break;
                }
                if matches!(*exit.borrow(), Some(Ok(()))) {
                    break;
                }
                tokio::select! {
                    changed=owner.changed()=>if changed.is_err() {native.request_close();break;},
                    changed=exit.changed()=>if changed.is_err() {break;},
                }
            }
        });
    }
}
fn failure(reason: &str) -> DriverError {
    DriverError::Disconnected(format!("Codex {reason}"))
}
