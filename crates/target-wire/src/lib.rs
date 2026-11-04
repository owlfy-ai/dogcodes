//! Frames exchanged between the desktop and `dogcodes-shim` over one SSH channel.
//!
//! The shim is uploaded by the same app build that talks to it, so there is no
//! compatibility layer: a version mismatch is a deployment fault, not a dialect.
use serde::{Deserialize, Serialize};
use std::io;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const PROTOCOL_VERSION: u32 = 1;
/// Printed once by `serve`, after which stdout carries only frames.
pub const READY_PREFIX: &str = "DOGCODES-SHIM-READY";
/// Printed by `--self-check` with the SHA-256 of the running executable.
pub const CHECK_PREFIX: &str = "DOGCODES-SHIM-CHECK";
pub const MAX_PAYLOAD: usize = 1024 * 1024;
pub const CONTROL_STREAM: u32 = 0;
const HEADER: usize = 9;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum FrameKind {
    Control = 0,
}

impl FrameKind {
    fn parse(value: u8) -> io::Result<Self> {
        match value {
            0 => Ok(Self::Control),
            other => Err(invalid(format!("unknown frame kind {other}"))),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Frame {
    pub stream: u32,
    pub kind: FrameKind,
    pub payload: Vec<u8>,
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

/// `Ok(None)` is a clean end of stream at a frame boundary. Not cancellation
/// safe: a dropped read loses its position, so callers own one reader task.
pub async fn read_frame<R: AsyncRead + Unpin>(reader: &mut R) -> io::Result<Option<Frame>> {
    let mut header = [0_u8; HEADER];
    let mut filled = 0;
    while filled < HEADER {
        let count = reader.read(&mut header[filled..]).await?;
        if count == 0 {
            return if filled == 0 {
                Ok(None)
            } else {
                Err(io::ErrorKind::UnexpectedEof.into())
            };
        }
        filled += count;
    }
    let length = u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as usize;
    if length > MAX_PAYLOAD {
        return Err(invalid(format!(
            "frame of {length} bytes exceeds the limit"
        )));
    }
    let stream = u32::from_be_bytes([header[4], header[5], header[6], header[7]]);
    let kind = FrameKind::parse(header[8])?;
    let mut payload = vec![0; length];
    reader.read_exact(&mut payload).await?;
    Ok(Some(Frame {
        stream,
        kind,
        payload,
    }))
}

pub async fn write_frame<W: AsyncWrite + Unpin>(writer: &mut W, frame: &Frame) -> io::Result<()> {
    if frame.payload.len() > MAX_PAYLOAD {
        return Err(invalid("frame payload exceeds the limit"));
    }
    let length = u32::try_from(frame.payload.len()).map_err(|_| invalid("frame too large"))?;
    let mut bytes = Vec::with_capacity(HEADER + frame.payload.len());
    bytes.extend_from_slice(&length.to_be_bytes());
    bytes.extend_from_slice(&frame.stream.to_be_bytes());
    bytes.push(frame.kind as u8);
    bytes.extend_from_slice(&frame.payload);
    writer.write_all(&bytes).await?;
    writer.flush().await
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    Ping,
    Info,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RequestEnvelope {
    pub id: u64,
    pub request: Request,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    Pong,
    Info(TargetInfo),
    Error { message: String },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResponseEnvelope {
    pub id: u64,
    pub response: Response,
}

/// What the desktop needs to know about the machine and account it reached.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetInfo {
    pub protocol: u32,
    pub version: String,
    pub os: String,
    pub arch: String,
    pub home: String,
    pub shell: Option<String>,
    /// Stable hash of the machine and the account, never the raw machine id.
    pub machine_identity: String,
}

pub fn control_frame<T: Serialize>(value: &T) -> io::Result<Frame> {
    Ok(Frame {
        stream: CONTROL_STREAM,
        kind: FrameKind::Control,
        payload: serde_json::to_vec(value).map_err(|error| invalid(error.to_string()))?,
    })
}

pub fn parse_control<T: for<'de> Deserialize<'de>>(frame: &Frame) -> io::Result<T> {
    if frame.kind != FrameKind::Control || frame.stream != CONTROL_STREAM {
        return Err(invalid("unexpected frame on the control stream"));
    }
    serde_json::from_slice(&frame.payload).map_err(|error| invalid(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn frames_round_trip_and_end_cleanly_at_a_boundary() {
        let (mut left, mut right) = tokio::io::duplex(4096);
        let frame = control_frame(&RequestEnvelope {
            id: 7,
            request: Request::Ping,
        })
        .unwrap();
        write_frame(&mut left, &frame).await.unwrap();
        drop(left);
        let read = read_frame(&mut right).await.unwrap().unwrap();
        assert_eq!(read, frame);
        let envelope: RequestEnvelope = parse_control(&read).unwrap();
        assert_eq!(envelope.id, 7);
        assert!(read_frame(&mut right).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn a_stream_cut_inside_a_frame_is_an_error_not_a_clean_end() {
        let (mut left, mut right) = tokio::io::duplex(64);
        left.write_all(&[0, 0, 0, 5, 0]).await.unwrap();
        drop(left);
        assert!(read_frame(&mut right).await.is_err());
    }

    #[tokio::test]
    async fn oversized_and_unknown_frames_are_rejected_before_allocation() {
        let (mut left, mut right) = tokio::io::duplex(64);
        left.write_all(&[0xff, 0xff, 0xff, 0xff, 0, 0, 0, 0, 0])
            .await
            .unwrap();
        assert!(read_frame(&mut right).await.is_err());

        let (mut left, mut right) = tokio::io::duplex(64);
        left.write_all(&[0, 0, 0, 0, 0, 0, 0, 0, 9]).await.unwrap();
        assert!(read_frame(&mut right).await.is_err());
    }

    #[tokio::test]
    async fn an_oversized_payload_is_not_written() {
        let (mut left, _right) = tokio::io::duplex(64);
        let frame = Frame {
            stream: 0,
            kind: FrameKind::Control,
            payload: vec![0; MAX_PAYLOAD + 1],
        };
        assert!(write_frame(&mut left, &frame).await.is_err());
    }
}
