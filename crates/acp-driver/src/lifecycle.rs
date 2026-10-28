//! Independently owned native scopes and the tasks using an ACP connection.
use agent_runtime::OwnedChild;
use driver_api::DriverError;
use std::{
    future::Future,
    io,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    process::{ChildStdin, ChildStdout},
    sync::watch,
    task::{JoinHandle, JoinSet},
};
type Outcome = Option<Result<(), DriverError>>;

pub(super) struct Control {
    stopping: AtomicBool,
    close: watch::Sender<bool>,
    exit: watch::Receiver<Outcome>,
}
pub(super) struct Process {
    pub control: Arc<Control>,
}
impl Process {
    pub fn start(child: OwnedChild, stderr: Option<JoinHandle<()>>) -> Self {
        Self::start_with_grace(child, stderr, Duration::from_secs(5))
    }
    pub fn start_with_grace(
        mut child: OwnedChild,
        stderr: Option<JoinHandle<()>>,
        grace: Duration,
    ) -> Self {
        let (close, mut closing) = watch::channel(false);
        let (completed, exit) = watch::channel(None);
        let control = Arc::new(Control {
            stopping: AtomicBool::new(false),
            close,
            exit,
        });
        let actor = control.clone();
        tokio::spawn(async move {
            tokio::select! {
                ()=async{while !*closing.borrow_and_update(){if closing.changed().await.is_err(){break;}}}=>{},
                _=child.wait()=>{},
            }
            actor.stopping.store(true, Ordering::Release);
            if !matches!(tokio::time::timeout(grace, child.wait()).await, Ok(Ok(_))) {
                let mut delay = 1;
                while child.terminate().await.is_err() {
                    completed
                        .send_replace(Some(Err(failure("native scope exit remains unconfirmed"))));
                    tokio::time::sleep(Duration::from_secs(delay)).await;
                    delay = (delay * 2).min(30);
                }
            }
            if let Some(stderr) = stderr {
                stderr.abort();
                let _ = stderr.await;
            }
            completed.send_replace(Some(Ok(())));
        });
        Self { control }
    }
    #[cfg(test)]
    pub async fn wait(&mut self) -> Result<(), DriverError> {
        self.control.closed().await
    }
    pub async fn kill(&mut self) -> Result<(), DriverError> {
        self.control.request_close();
        self.control.closed().await
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        self.control.request_close();
    }
}
impl Control {
    pub fn stopping(&self) -> bool {
        self.stopping.load(Ordering::Acquire)
    }
    pub fn request_close(&self) {
        self.stopping.store(true, Ordering::Release);
        self.close.send_replace(true);
    }
    pub async fn closing(&self) {
        let mut close = self.close.subscribe();
        while !self.stopping() {
            tokio::select! {
                _=close.changed()=>{},
                ()=tokio::time::sleep(Duration::from_millis(10))=>{},
            }
        }
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
        .map_err(|_| failure("native exit is still pending confirmation"))?
    }
    pub fn observe_owner(self: &Arc<Self>, mut owner: watch::Receiver<bool>) {
        let control = self.clone();
        tokio::spawn(async move {
            let mut exit = control.exit.clone();
            loop {
                if *owner.borrow() {
                    control.request_close();
                    break;
                }
                if matches!(*exit.borrow(), Some(Ok(()))) {
                    break;
                }
                tokio::select! {
                    changed=owner.changed()=>if changed.is_err(){control.request_close();break;},
                    changed=exit.changed()=>if changed.is_err(){break;},
                }
            }
        });
    }
}
fn failure(reason: &str) -> DriverError {
    DriverError::Disconnected(format!("ACP {reason}"))
}

pub(super) struct Input {
    pub inner: ChildStdin,
    pub control: Arc<Control>,
}
impl AsyncWrite for Input {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if this.control.stopping() {
            return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()));
        }
        let result = Pin::new(&mut this.inner).poll_write(cx, bytes);
        if matches!(result, Poll::Ready(Err(_))) {
            this.control.request_close();
        }
        result
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let result = Pin::new(&mut this.inner).poll_flush(cx);
        if matches!(result, Poll::Ready(Err(_))) {
            this.control.request_close();
        }
        result
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let result = Pin::new(&mut this.inner).poll_shutdown(cx);
        if result.is_ready() {
            this.control.request_close();
        }
        result
    }
}
impl Drop for Input {
    fn drop(&mut self) {
        self.control.request_close();
    }
}
pub(super) struct Output {
    pub inner: ChildStdout,
    pub control: Arc<Control>,
    pub line_bytes: usize,
}
impl AsyncRead for Output {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if buffer.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        let this = self.get_mut();
        let before = buffer.filled().len();
        let result = Pin::new(&mut this.inner).poll_read(cx, buffer);
        if matches!(result, Poll::Ready(Err(_))) {
            this.control.request_close();
        }
        if matches!(result, Poll::Ready(Ok(()))) {
            let bytes = &buffer.filled()[before..];
            if bytes.is_empty() {
                this.control.request_close();
            }
            for byte in bytes {
                if *byte == b'\n' {
                    this.line_bytes = 0;
                } else {
                    this.line_bytes += 1;
                }
                if this.line_bytes > 8 * 1024 * 1024 {
                    this.control.request_close();
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "ACP frame exceeds limit",
                    )));
                }
            }
        }
        result
    }
}

pub(super) struct Tasks {
    set: Mutex<Option<JoinSet<()>>>,
}
impl Tasks {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            set: Mutex::new(Some(JoinSet::new())),
        })
    }
    pub fn spawn(&self, future: impl Future<Output = ()> + Send + 'static) {
        let mut slot = self.set.lock().expect("ACP task group");
        if let Some(set) = slot.as_mut() {
            while set.try_join_next().is_some() {}
            set.spawn(future);
        }
    }
    pub async fn shutdown(&self) {
        let set = self.set.lock().expect("ACP task group").take();
        if let Some(mut set) = set {
            set.abort_all();
            while set.join_next().await.is_some() {}
        }
    }
}
