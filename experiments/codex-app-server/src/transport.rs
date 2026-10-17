//! Bounded JSONL transport used by the M0 probe.

use serde_json::{Value, json};
use thiserror::Error;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

const MAX_INTERLEAVED_FRAMES: usize = 256;

#[derive(Clone, Debug, PartialEq)]
pub enum IncomingFrame {
    Response {
        id: Value,
        result: Option<Value>,
        error: Option<Value>,
        raw: Value,
    },
    Notification {
        method: String,
        params: Value,
        raw: Value,
    },
    ServerRequest {
        id: Value,
        method: String,
        params: Value,
        raw: Value,
    },
    Unknown {
        raw: Value,
    },
}

impl IncomingFrame {
    #[must_use]
    pub fn raw(&self) -> &Value {
        match self {
            Self::Response { raw, .. }
            | Self::Notification { raw, .. }
            | Self::ServerRequest { raw, .. }
            | Self::Unknown { raw } => raw,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RequestExchange {
    pub response: IncomingFrame,
    pub interleaved: Vec<IncomingFrame>,
}

pub struct JsonlTransport<R, W> {
    reader: BufReader<R>,
    writer: W,
    max_frame_bytes: usize,
}

impl<R, W> JsonlTransport<R, W>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    #[must_use]
    pub fn new(reader: R, writer: W, max_frame_bytes: usize) -> Self {
        Self {
            reader: BufReader::new(reader),
            writer,
            max_frame_bytes,
        }
    }

    pub async fn send_notification(
        &mut self,
        method: &str,
        params: Option<Value>,
    ) -> Result<(), TransportError> {
        let mut frame = json!({ "method": method });
        if let Some(params) = params {
            frame["params"] = params;
        }
        self.send_frame(&frame).await
    }

    pub async fn request(
        &mut self,
        id: Value,
        method: &str,
        params: Value,
    ) -> Result<RequestExchange, TransportError> {
        require_request_id(&id)?;
        self.send_frame(&json!({
            "id": id.clone(),
            "method": method,
            "params": params,
        }))
        .await?;

        let mut interleaved = Vec::new();
        loop {
            let frame = self.read_frame().await?;
            let matches = matches!(
                &frame,
                IncomingFrame::Response { id: response_id, .. } if response_id == &id
            );
            if matches {
                return Ok(RequestExchange {
                    response: frame,
                    interleaved,
                });
            }
            if interleaved.len() >= MAX_INTERLEAVED_FRAMES {
                return Err(TransportError::InterleavedOverflow {
                    limit: MAX_INTERLEAVED_FRAMES,
                });
            }
            interleaved.push(frame);
        }
    }

    pub async fn send_response(&mut self, id: Value, result: Value) -> Result<(), TransportError> {
        require_request_id(&id)?;
        self.send_frame(&json!({ "id": id, "result": result }))
            .await
    }

    pub async fn send_frame(&mut self, frame: &Value) -> Result<(), TransportError> {
        let bytes = serde_json::to_vec(frame)?;
        if bytes.len() > self.max_frame_bytes {
            return Err(TransportError::OversizedFrame {
                limit: self.max_frame_bytes,
            });
        }
        self.writer.write_all(&bytes).await?;
        self.writer.write_all(b"\n").await?;
        self.writer.flush().await?;
        Ok(())
    }

    pub async fn read_frame(&mut self) -> Result<IncomingFrame, TransportError> {
        let bytes = read_bounded_line(&mut self.reader, self.max_frame_bytes).await?;
        classify_frame(serde_json::from_slice(&bytes)?)
    }
}

pub fn classify_frame(raw: Value) -> Result<IncomingFrame, TransportError> {
    let Some(object) = raw.as_object() else {
        return Ok(IncomingFrame::Unknown { raw });
    };
    let id = object.get("id").cloned();
    let method = object.get("method").and_then(Value::as_str);
    let params = object.get("params").cloned().unwrap_or(Value::Null);

    match (id, method) {
        (Some(id), Some(method)) => {
            require_request_id(&id)?;
            Ok(IncomingFrame::ServerRequest {
                id,
                method: method.to_owned(),
                params,
                raw,
            })
        }
        (None, Some(method)) => Ok(IncomingFrame::Notification {
            method: method.to_owned(),
            params,
            raw,
        }),
        (Some(id), None) if object.contains_key("result") || object.contains_key("error") => {
            require_request_id(&id)?;
            Ok(IncomingFrame::Response {
                id,
                result: object.get("result").cloned(),
                error: object.get("error").cloned(),
                raw,
            })
        }
        _ => Ok(IncomingFrame::Unknown { raw }),
    }
}

async fn read_bounded_line<R>(
    reader: &mut R,
    max_frame_bytes: usize,
) -> Result<Vec<u8>, TransportError>
where
    R: AsyncBufRead + Unpin,
{
    let mut line = Vec::with_capacity(max_frame_bytes.min(8 * 1_024));
    loop {
        let consumed;
        let complete;
        {
            let available = reader.fill_buf().await?;
            if available.is_empty() {
                if line.is_empty() {
                    return Err(TransportError::Closed);
                }
                return Ok(line);
            }
            let newline = available.iter().position(|byte| *byte == b'\n');
            let payload_len = newline.unwrap_or(available.len());
            if line.len().saturating_add(payload_len) > max_frame_bytes {
                return Err(TransportError::OversizedFrame {
                    limit: max_frame_bytes,
                });
            }
            line.extend_from_slice(&available[..payload_len]);
            complete = newline.is_some();
            consumed = payload_len + usize::from(complete);
        }
        reader.consume(consumed);
        if complete {
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            return Ok(line);
        }
    }
}

fn require_request_id(id: &Value) -> Result<(), TransportError> {
    if id.is_string() || id.is_number() {
        Ok(())
    } else {
        Err(TransportError::InvalidRequestId)
    }
}

#[derive(Debug, Error)]
pub enum TransportError {
    #[error("app-server transport closed")]
    Closed,
    #[error("app-server frame exceeded {limit} bytes")]
    OversizedFrame { limit: usize },
    #[error("request id must be a string or number")]
    InvalidRequestId,
    #[error("more than {limit} frames arrived before the matching response")]
    InterleavedOverflow { limit: usize },
    #[error("app-server transport I/O failed: {0}")]
    Io(String),
    #[error("app-server emitted invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
}

impl From<std::io::Error> for TransportError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncBufReadExt, BufReader, duplex, split};

    use super::*;

    #[test]
    fn classifies_all_protocol_frame_shapes_without_dropping_raw_fields() {
        let notification = json!({"method":"turn/started","params":{"turn":{"id":"t"}},"extra":1});
        let classified = classify_frame(notification.clone()).unwrap();
        assert!(matches!(classified, IncomingFrame::Notification { .. }));
        assert_eq!(classified.raw(), &notification);

        let request = classify_frame(json!({
            "id": "approval-1",
            "method": "item/commandExecution/requestApproval",
            "params": {"itemId":"item-1"}
        }))
        .unwrap();
        assert!(matches!(request, IncomingFrame::ServerRequest { .. }));

        let response = classify_frame(json!({"id":1,"result":{"ok":true}})).unwrap();
        assert!(matches!(response, IncomingFrame::Response { .. }));
        assert!(matches!(
            classify_frame(json!([1, 2])).unwrap(),
            IncomingFrame::Unknown { .. }
        ));
    }

    #[tokio::test]
    async fn request_preserves_interleaved_notifications() {
        let (client, server) = duplex(4 * 1_024);
        let (client_read, client_write) = split(client);
        let (server_read, mut server_write) = split(server);
        let server_task = tokio::spawn(async move {
            let mut request = String::new();
            BufReader::new(server_read)
                .read_line(&mut request)
                .await
                .unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(&request).unwrap()["method"],
                "initialize"
            );
            server_write
                .write_all(b"{\"method\":\"configWarning\",\"params\":{\"summary\":\"safe\"}}\n")
                .await
                .unwrap();
            server_write
                .write_all(b"{\"id\":1,\"result\":{\"userAgent\":\"fake\"}}\n")
                .await
                .unwrap();
        });
        let mut transport = JsonlTransport::new(client_read, client_write, 4 * 1_024);
        let exchange = transport
            .request(json!(1), "initialize", json!({"clientInfo":{}}))
            .await
            .unwrap();
        assert_eq!(exchange.interleaved.len(), 1);
        assert!(matches!(
            exchange.interleaved[0],
            IncomingFrame::Notification { ref method, .. } if method == "configWarning"
        ));
        assert!(matches!(exchange.response, IncomingFrame::Response { .. }));
        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn rejects_oversized_inbound_and_outbound_frames() {
        let (client, mut server) = duplex(128);
        let (client_read, client_write) = split(client);
        let mut transport = JsonlTransport::new(client_read, client_write, 16);
        server
            .write_all(b"{\"method\":\"1234567890123456\"}\n")
            .await
            .unwrap();
        assert!(matches!(
            transport.read_frame().await,
            Err(TransportError::OversizedFrame { limit: 16 })
        ));
        assert!(matches!(
            transport.send_frame(&json!({"payload":"too long"})).await,
            Err(TransportError::OversizedFrame { limit: 16 })
        ));
    }
}
