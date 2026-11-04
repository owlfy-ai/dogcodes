//! The live control channel to a started shim.
use crate::error::{ConnectError, ConnectErrorKind};
use agent_runtime::OwnedChild;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use target_wire::{
    PROTOCOL_VERSION, Request, RequestEnvelope, Response, ResponseEnvelope, TargetInfo,
    control_frame, parse_control, read_frame, write_frame,
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::{mpsc, oneshot, watch},
};

const PING_INTERVAL: Duration = Duration::from_secs(15);
/// Long enough to ride out what `ServerAlive*` already tolerates.
const REPLY_LIMIT: Duration = Duration::from_mins(1);
const HANDSHAKE_LIMIT: Duration = Duration::from_secs(20);

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Response>>>>;

pub struct TargetLink {
    info: TargetInfo,
    requests: mpsc::Sender<(Request, oneshot::Sender<Response>)>,
    closed: watch::Receiver<Option<String>>,
}

impl TargetLink {
    /// Dropping the link ends the connection: with its request channel closed the
    /// worker stops the session and confirms the `ssh` process has gone.
    pub(crate) async fn start<R, W>(
        child: OwnedChild,
        reader: R,
        writer: W,
        stderr: Arc<Mutex<String>>,
    ) -> Result<Self, ConnectError>
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let (requests, inbox) = mpsc::channel(16);
        let (close, closed) = watch::channel(None);
        tokio::spawn(run(child, reader, writer, inbox, close, stderr));
        let (reply, answer) = oneshot::channel();
        let failed = |detail: &str| ConnectError::new(ConnectErrorKind::ProtocolMismatch, detail);
        requests
            .send((Request::Info, reply))
            .await
            .map_err(|_| failed("shim channel closed"))?;
        let response = tokio::time::timeout(HANDSHAKE_LIMIT, answer)
            .await
            .map_err(|_| failed("shim did not answer"))?
            .map_err(|_| failed("shim channel closed"))?;
        let Response::Info(info) = response else {
            return Err(failed("unexpected reply to the first request"));
        };
        if info.protocol != PROTOCOL_VERSION {
            return Err(failed("shim protocol differs from this app"));
        }
        Ok(Self {
            info,
            requests,
            closed,
        })
    }

    #[must_use]
    pub fn info(&self) -> &TargetInfo {
        &self.info
    }

    /// Becomes `Some(reason)` once, when the channel is gone for any reason.
    #[must_use]
    pub fn closed(&self) -> watch::Receiver<Option<String>> {
        self.closed.clone()
    }

    pub async fn ping(&self) -> Result<(), String> {
        let (reply, answer) = oneshot::channel();
        self.requests
            .send((Request::Ping, reply))
            .await
            .map_err(|_| "connection closed".to_owned())?;
        match tokio::time::timeout(REPLY_LIMIT, answer).await {
            Ok(Ok(Response::Pong)) => Ok(()),
            Ok(Ok(_)) => Err("unexpected reply".into()),
            Ok(Err(_)) => Err("connection closed".into()),
            Err(_) => Err("timed out".into()),
        }
    }
}

async fn run<R, W>(
    mut child: OwnedChild,
    mut reader: R,
    mut writer: W,
    mut inbox: mpsc::Receiver<(Request, oneshot::Sender<Response>)>,
    close: watch::Sender<Option<String>>,
    stderr: Arc<Mutex<String>>,
) where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let pending: Pending = Arc::default();
    let (ended, mut reader_ended) = oneshot::channel::<String>();
    let replies = pending.clone();
    // One owner for the read position: `read_frame` must not be cancelled midway.
    let reader_task = tokio::spawn(async move {
        let reason = loop {
            let frame = match read_frame(&mut reader).await {
                Ok(Some(frame)) => frame,
                Ok(None) => break "connection closed".to_owned(),
                Err(error) => break error.to_string(),
            };
            let envelope: ResponseEnvelope = match parse_control(&frame) {
                Ok(envelope) => envelope,
                Err(error) => break error.to_string(),
            };
            if let Some(reply) = replies.lock().unwrap().remove(&envelope.id) {
                let _ = reply.send(envelope.response);
            }
        };
        let _ = ended.send(reason);
    });
    let mut next_id = 0_u64;
    let mut heartbeat = tokio::time::interval(PING_INTERVAL);
    heartbeat.tick().await;
    let mut unanswered_since: Option<tokio::time::Instant> = None;
    let mut heartbeat_reply: Option<oneshot::Receiver<Response>> = None;
    let reason = loop {
        tokio::select! {
            reason = &mut reader_ended => break reason.unwrap_or_else(|_| "connection closed".into()),
            message = inbox.recv() => {
                let Some((request, reply)) = message else {
                    break "closed by this app".into();
                };
                next_id += 1;
                pending.lock().unwrap().insert(next_id, reply);
                if let Err(error) = send(&mut writer, next_id, request).await {
                    break error;
                }
            }
            _ = heartbeat.tick() => {
                if let Some(receiver) = &mut heartbeat_reply {
                    if receiver.try_recv().is_ok() {
                        unanswered_since = None;
                    } else if unanswered_since.is_some_and(|since| since.elapsed() > REPLY_LIMIT) {
                        break "the target stopped responding".into();
                    }
                }
                let (reply, answer) = oneshot::channel();
                next_id += 1;
                pending.lock().unwrap().insert(next_id, reply);
                heartbeat_reply = Some(answer);
                unanswered_since.get_or_insert_with(tokio::time::Instant::now);
                if let Err(error) = send(&mut writer, next_id, Request::Ping).await {
                    break error;
                }
            }
        }
    };
    reader_task.abort();
    // Closing stdin lets the shim end by itself before the process is signalled.
    drop(writer);
    let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
    let _ = child.terminate().await;
    let detail = stderr.lock().unwrap().trim().to_owned();
    let message = if detail.is_empty() {
        reason
    } else {
        format!("{reason}: {detail}")
    };
    let _ = close.send(Some(message));
}

async fn send<W: AsyncWrite + Unpin>(
    writer: &mut W,
    id: u64,
    request: Request,
) -> Result<(), String> {
    let frame =
        control_frame(&RequestEnvelope { id, request }).map_err(|error| error.to_string())?;
    write_frame(writer, &frame)
        .await
        .map_err(|error| error.to_string())
}
