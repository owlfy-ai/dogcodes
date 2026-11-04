//! Frames exchanged between the desktop and `dogcodes-shim` over one SSH channel.
//!
//! The shim is uploaded by the same app build that talks to it, so there is no
//! compatibility layer: a version mismatch is a deployment fault, not a dialect.
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, io};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const PROTOCOL_VERSION: u32 = 2;
/// Printed once by `serve`, after which stdout carries only frames.
pub const READY_PREFIX: &str = "DOGCODES-SHIM-READY";
/// Printed by `--self-check` with the SHA-256 of the running executable.
pub const CHECK_PREFIX: &str = "DOGCODES-SHIM-CHECK";
pub const MAX_PAYLOAD: usize = 1024 * 1024;
pub const CONTROL_STREAM: u32 = 0;
const HEADER: usize = 9;
/// Unacknowledged bytes allowed on one process stream, so a slow reader stops its
/// writer without blocking the control channel or any other process.
pub const STREAM_WINDOW: u32 = 256 * 1024;
pub const STREAM_CHUNK: u32 = 32 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum FrameKind {
    Control = 0,
    Data = 1,
    Eof = 2,
    /// The receiver consumed this many bytes (big-endian `u32` payload).
    Credit = 3,
}

impl FrameKind {
    fn parse(value: u8) -> io::Result<Self> {
        match value {
            0 => Ok(Self::Control),
            1 => Ok(Self::Data),
            2 => Ok(Self::Eof),
            3 => Ok(Self::Credit),
            other => Err(invalid(format!("unknown frame kind {other}"))),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Channel {
    Stdin = 0,
    Stdout = 1,
    Stderr = 2,
}

/// Process streams start after the control stream.
#[must_use]
pub fn stream_id(process: u32, channel: Channel) -> u32 {
    process * 4 + channel as u32
}

#[must_use]
pub fn split_stream(stream: u32) -> Option<(u32, Channel)> {
    let channel = match stream % 4 {
        0 => Channel::Stdin,
        1 => Channel::Stdout,
        2 => Channel::Stderr,
        _ => return None,
    };
    let process = stream / 4;
    (process > 0).then_some((process, channel))
}

#[must_use]
pub fn data_frame(stream: u32, payload: Vec<u8>) -> Frame {
    Frame {
        stream,
        kind: FrameKind::Data,
        payload,
    }
}

#[must_use]
pub fn eof_frame(stream: u32) -> Frame {
    Frame {
        stream,
        kind: FrameKind::Eof,
        payload: vec![],
    }
}

#[must_use]
pub fn credit_frame(stream: u32, bytes: u32) -> Frame {
    Frame {
        stream,
        kind: FrameKind::Credit,
        payload: bytes.to_be_bytes().to_vec(),
    }
}

#[must_use]
pub fn parse_credit(frame: &Frame) -> Option<u32> {
    Some(u32::from_be_bytes(
        frame.payload.as_slice().try_into().ok()?,
    ))
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
    /// The account's environment as its login shell sets it up.
    LoginEnvironment,
    Spawn(SpawnRequest),
    Terminate {
        process: u32,
    },
}

/// The process environment is exactly `env`: nothing is inherited from the shim.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SpawnRequest {
    /// Chosen by the desktop, so streams can be registered before the process starts.
    pub process: u32,
    pub program: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub cwd: Option<String>,
    pub stdin: bool,
    pub stdout: bool,
    pub stderr: bool,
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
    Ok,
    Info(TargetInfo),
    Environment { values: BTreeMap<String, String> },
    Error { message: String },
}

/// Everything the shim sends on the control stream.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ShimMessage {
    Response(ResponseEnvelope),
    /// The process group has ended, or the process never started (`error`).
    Exited {
        process: u32,
        code: Option<i32>,
        signal: Option<i32>,
        error: Option<String>,
    },
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

    #[test]
    fn process_streams_never_collide_with_control() {
        assert_eq!(split_stream(CONTROL_STREAM), None);
        assert_eq!(split_stream(3), None);
        for process in [1, 7, 1000] {
            for channel in [Channel::Stdin, Channel::Stdout, Channel::Stderr] {
                assert_eq!(
                    split_stream(stream_id(process, channel)),
                    Some((process, channel))
                );
            }
        }
        assert_eq!(split_stream(stream_id(1, Channel::Stdin) + 3), None);
    }

    #[test]
    fn a_malformed_credit_is_refused_rather_than_read_as_zero() {
        assert_eq!(parse_credit(&credit_frame(4, 9)), Some(9));
        assert_eq!(parse_credit(&eof_frame(4)), None);
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
