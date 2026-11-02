//! Full-duplex transport and bounded message scheduling. Fragment boundaries
//! are authenticated messages, so control replies can pass an incomplete frame.
use super::{Session, Socket, Value, WebSocket, failure, json, native};
use futures_util::{
    Sink, SinkExt, Stream, StreamExt,
    stream::{SplitSink, SplitStream},
};
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, atomic::Ordering},
    time::Duration,
};
use tokio::sync::{Mutex, mpsc};
use tokio_tungstenite::tungstenite::Message;

const CHUNK: usize = 8192;
const SMALL_QUEUE: usize = 16;
const LARGE_QUEUE: usize = 4;
const BULK_WINDOW: usize = 32 * 1024;
const MAX_FLIGHTS: usize = 16;
pub(super) type Noise<'a> = Arc<Mutex<&'a mut Session>>;

pub(super) enum Reader<S = WebSocket> {
    Relay(SplitStream<S>),
    Direct(Arc<dogcodes_remote_transport::Channel>),
}
enum Output<S = WebSocket> {
    Relay(SplitSink<S, Message>),
    Direct(Arc<dogcodes_remote_transport::Channel>),
}
pub(super) fn split(socket: Socket) -> (Reader, WriterTransport) {
    match socket {
        Socket::Relay(socket) => split_relay(*socket),
        Socket::Direct(channel) => {
            let channel = Arc::new(channel);
            (
                Reader::Direct(channel.clone()),
                WriterTransport(Output::Direct(channel)),
            )
        }
    }
}
fn split_relay<S>(socket: S) -> (Reader<S>, WriterTransport<S>)
where
    S: Stream + Sink<Message>,
{
    let (sink, stream) = socket.split();
    (Reader::Relay(stream), WriterTransport(Output::Relay(sink)))
}
impl<S> Reader<S>
where
    S: Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    pub(super) async fn next(&mut self) -> Option<Result<Message, String>> {
        match self {
            Self::Relay(socket) => socket.next().await.map(|v| v.map_err(failure)),
            Self::Direct(socket) => match socket.receive().await {
                Ok(Some(bytes)) => Some(Ok(Message::Binary(bytes.into()))),
                Ok(None) => None,
                Err(error) => Some(Err(error)),
            },
        }
    }
    pub(super) async fn status(&self) -> Result<Value, String> {
        match self {
            Self::Relay(_) => Ok(json!({"transport":"wss-relay","mode":"server-relay"})),
            Self::Direct(socket) => tokio::time::timeout(Duration::from_secs(2), socket.route())
                .await
                .map(|route| json!({"transport":"webrtc-datachannel","route":route}))
                .map_err(|_| "连接诊断暂时不可用".into()),
        }
    }
}
pub(super) struct WriterTransport<S = WebSocket>(Output<S>);
impl<S> WriterTransport<S>
where
    S: Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    async fn send(&mut self, message: Message) -> Result<(), String> {
        let send = async {
            match &mut self.0 {
                Output::Relay(socket) => socket.send(message).await.map_err(failure),
                Output::Direct(socket) => match message {
                    Message::Binary(bytes) => socket.send(&bytes).await,
                    Message::Close(_) => socket.close().await,
                    Message::Pong(_) => Ok(()),
                    _ => Err("invalid direct output".into()),
                },
            }
        };
        tokio::time::timeout(Duration::from_secs(5), send)
            .await
            .map_err(|_| "slow consumer")?
    }
    pub(super) async fn close(&mut self) {
        let _ = tokio::time::timeout(Duration::from_secs(1), self.send(Message::Close(None))).await;
    }
    async fn frame(&mut self, noise: &Noise<'_>, frame: &[u8]) -> Result<(), String> {
        for part in frame.chunks(dogcodes_remote_transport::PLAINTEXT_CHUNK) {
            // Never hold the cryptographic state while waiting for the network.
            let encrypted = noise.lock().await.seal(part).map_err(failure)?;
            self.send(Message::Binary(encrypted.into())).await?;
        }
        Ok(())
    }
}

struct Pending {
    bytes: Vec<u8>,
    id: String,
    offset: usize,
    chunked: bool,
    generation: Option<u64>,
}
impl Pending {
    fn new(bytes: Vec<u8>, chunked: bool) -> Self {
        Self {
            bytes,
            id: uuid::Uuid::new_v4().simple().to_string(),
            offset: 0,
            chunked,
            generation: None,
        }
    }
    fn next(&mut self) -> Result<Vec<u8>, String> {
        if !self.chunked {
            self.offset = self.bytes.len();
            return Ok(std::mem::take(&mut self.bytes));
        }
        let end = (self.offset + CHUNK).min(self.bytes.len());
        let framed = super::binary_frame(
            &json!({"type":"fragment","id":self.id,
            "offset":self.offset,"total":self.bytes.len()}),
            &self.bytes[self.offset..end],
        )?;
        self.offset = end;
        Ok(framed)
    }
    fn done(&self) -> bool {
        self.bytes.is_empty() || self.offset == self.bytes.len()
    }
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Progress {
    id: String,
    offset: usize,
}
pub(super) fn progress(value: Value) -> Result<Vec<Progress>, String> {
    let progress: Vec<Progress> =
        serde_json::from_value(value).map_err(|_| "invalid fragment acknowledgement")?;
    if progress.is_empty()
        || progress.len() > MAX_FLIGHTS
        || progress.iter().any(|p| {
            p.id.len() != 32
                || !p
                    .id
                    .bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
                || p.offset > super::MAX_FRAME_BYTES + 4
        })
    {
        return Err("invalid fragment acknowledgement".into());
    }
    Ok(progress)
}
pub(super) fn configure(connection: &native::Connection, args: &Value) -> Result<Value, String> {
    if args["fragmentedMessages"] != true {
        return Err("fragment negotiation requires fragmentedMessages=true".into());
    }
    connection.chunked.store(true, Ordering::Release);
    Ok(
        json!({"fragmentedMessages":true,"fragmentAcks":true,"chunkBytes":CHUNK,"bulkWindowBytes":BULK_WINDOW,"maxAssemblies":2}),
    )
}
#[derive(Default)]
struct Flight {
    sent: usize,
    acked: usize,
    finished: bool,
}
#[derive(Default)]
struct Schedule {
    small: VecDeque<Vec<u8>>,
    large: VecDeque<Pending>,
    video: Option<Pending>,
    flights: HashMap<String, Flight>,
    bulk: usize,
    cancel: Option<Vec<u8>>,
}
impl Schedule {
    fn control(&mut self, bytes: Vec<u8>, chunked: bool) -> Result<(), String> {
        if bytes.len() <= CHUNK {
            if self.small.len() >= SMALL_QUEUE {
                return Err("control output queue full".into());
            }
            self.small.push_back(bytes);
        } else {
            if self.large.len() >= LARGE_QUEUE {
                return Err("large output queue full".into());
            }
            self.large.push_back(Pending::new(bytes, chunked));
        }
        Ok(())
    }
    fn discard_stale_video(&mut self, epoch: u64) -> Result<(), String> {
        if self
            .video
            .as_ref()
            .is_some_and(|p| p.generation.is_some_and(|g| g != epoch))
        {
            let pending = self.video.take().unwrap();
            if let Some(flight) = self.flights.remove(&pending.id) {
                self.bulk -= flight.sent - flight.acked;
            }
            if pending.chunked && pending.offset > 0 {
                if self.cancel.is_some() {
                    return Err("pending video cancellation".into());
                }
                self.cancel = Some(super::json_frame(
                    &json!({"type":"fragment_cancel","id":pending.id}),
                )?);
            }
        }
        Ok(())
    }
    fn fits(&self, pending: &Pending) -> bool {
        !pending.chunked
            || ((self.flights.contains_key(&pending.id) || self.flights.len() < MAX_FLIGHTS)
                && self.bulk + (pending.bytes.len() - pending.offset).min(CHUNK) <= BULK_WINDOW)
    }
    fn ready(&self) -> bool {
        self.cancel.is_some()
            || !self.small.is_empty()
            || self
                .large
                .front()
                .or(self.video.as_ref())
                .is_some_and(|p| self.fits(p))
    }
    fn acknowledge(&mut self, progress: Vec<Progress>) -> Result<(), String> {
        for p in progress {
            // A duplicate final ack can arrive after an entry was retired.
            let Some(flight) = self.flights.get_mut(&p.id) else {
                continue;
            };
            if p.offset > flight.sent {
                return Err("fragment acknowledgement exceeds sent bytes".into());
            }
            let offset = p.offset.max(flight.acked);
            self.bulk -= offset - flight.acked;
            flight.acked = offset;
            if flight.finished && flight.acked == flight.sent {
                self.flights.remove(&p.id);
            }
        }
        Ok(())
    }
    fn next(&mut self) -> Result<Vec<u8>, String> {
        if let Some(cancel) = self.cancel.take() {
            return Ok(cancel);
        }
        if let Some(small) = self.small.pop_front() {
            return Ok(small);
        }
        let pending = self
            .large
            .front()
            .or(self.video.as_ref())
            .ok_or("missing output")?;
        if !self.fits(pending) {
            return Err("fragment window full".into());
        }
        let control = !self.large.is_empty();
        let pending = if control {
            self.large.front_mut().unwrap()
        } else {
            self.video.as_mut().unwrap()
        };
        let before = pending.offset;
        let frame = pending.next()?;
        if pending.chunked {
            let flight = self.flights.entry(pending.id.clone()).or_default();
            self.bulk += pending.offset - before;
            flight.sent = pending.offset;
            flight.finished = pending.done();
        }
        if pending.done() {
            if control {
                self.large.pop_front();
            } else {
                self.video = None;
            }
        }
        Ok(frame)
    }
}

pub(super) struct Writer<'a> {
    pub transport: WriterTransport,
    pub noise: Noise<'a>,
    pub remote: &'a super::RemoteHost,
    pub lease: Arc<dogcodes_endpoint::pairing::DeviceLease>,
    pub connection: Arc<native::Connection>,
}
impl Writer<'_> {
    pub(super) async fn run(
        &mut self,
        mut replies: mpsc::Receiver<Value>,
        mut videos: mpsc::Receiver<(Value, Vec<u8>)>,
        mut pongs: mpsc::Receiver<Message>,
        mut acknowledgements: mpsc::Receiver<Vec<Progress>>,
    ) -> Result<(), String> {
        let mut schedule = Schedule::default();
        loop {
            schedule.discard_stale_video(self.connection.video_epoch.load(Ordering::Acquire))?;
            tokio::select! {
                progress=acknowledgements.recv()=>match progress {Some(progress)=>schedule.acknowledge(progress)?,None=>return Ok(())},
                pong=pongs.recv()=>match pong {
                    Some(pong)=>self.transport.send(pong).await?,None=>return Ok(()),
                },
                value=replies.recv(),if schedule.small.len()<SMALL_QUEUE=>match value {
                    Some(value)=>schedule.control(super::json_frame(&value)?,self.connection.chunked.load(Ordering::Acquire))?,
                    None=>return Ok(()),
                },
                frame=videos.recv(),if schedule.video.is_none()=>match frame {
                    Some((header,payload))=>{
                        let generation=header["generation"].as_u64();
                        if generation!=Some(self.connection.video_epoch.load(Ordering::Acquire)) {continue;}
                        let mut pending=Pending::new(super::binary_frame(&header,&payload)?,self.connection.chunked.load(Ordering::Acquire));
                        pending.generation=generation;
                        schedule.video=Some(pending);
                    },
                    None=>return Ok(()),
                },
                ()=async{},if schedule.ready()=>{
                    {
                        let state=self.remote.state.lock().await;
                        state.ready.as_ref().ok_or("remote unavailable")?.book.check(&self.lease).map_err(|_|"device revoked")?;
                    }
                    self.transport.frame(&self.noise,&schedule.next()?).await?;
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn switching_monitor_cancels_partial_old_video_and_releases_credit() {
        let mut pending = Pending::new(vec![7; 4 * 1024 * 1024], true);
        pending.generation = Some(1);
        let id = pending.id.clone();
        let mut schedule = Schedule {
            video: Some(pending),
            ..Schedule::default()
        };
        for _ in 0..BULK_WINDOW / CHUNK {
            schedule.next().unwrap();
        }
        assert!(!schedule.ready());
        schedule.discard_stale_video(2).unwrap();
        assert_eq!(schedule.bulk, 0);
        assert!(schedule.video.is_none());
        schedule
            .control(b"new monitor accepted".to_vec(), true)
            .unwrap();
        let cancel = schedule.next().unwrap();
        let cancel: Value = serde_json::from_slice(&cancel[4..]).unwrap();
        assert_eq!(cancel["type"], "fragment_cancel");
        assert_eq!(cancel["id"], id);
        assert_eq!(schedule.next().unwrap(), b"new monitor accepted");
        // Already queued delivery acknowledgements cannot revive the old frame.
        schedule
            .acknowledge(vec![Progress { id, offset: CHUNK }])
            .unwrap();
        assert_eq!(schedule.bulk, 0);
        assert!(schedule.flights.is_empty());
    }
    #[test]
    fn delivery_window_bounds_bulk_but_never_blocks_control() {
        let pending = Pending::new(vec![7; 4 * 1024 * 1024], true);
        let id = pending.id.clone();
        let mut schedule = Schedule {
            video: Some(pending),
            ..Schedule::default()
        };
        for _ in 0..BULK_WINDOW / CHUNK {
            schedule.next().unwrap();
        }
        assert_eq!(schedule.bulk, BULK_WINDOW);
        assert!(!schedule.ready());
        schedule.control(b"input released".to_vec(), true).unwrap();
        assert!(schedule.ready());
        assert_eq!(schedule.next().unwrap(), b"input released");
        schedule
            .acknowledge(vec![Progress {
                id: id.clone(),
                offset: CHUNK,
            }])
            .unwrap();
        assert!(schedule.ready());
        schedule.next().unwrap();
        assert!(!schedule.ready());
        // Duplicate/stale acknowledgements cannot manufacture credit.
        schedule
            .acknowledge(vec![Progress {
                id: id.clone(),
                offset: CHUNK,
            }])
            .unwrap();
        assert_eq!(schedule.bulk, BULK_WINDOW);
        assert!(
            schedule
                .acknowledge(vec![Progress {
                    id,
                    offset: CHUNK * 6
                }])
                .is_err()
        );
    }
    #[test]
    fn acknowledgement_metadata_and_negotiation_are_validated() {
        for value in [
            json!([]),
            json!([{"id":"bad","offset":0}]),
            json!([{"id":"a".repeat(32),"offset":-1}]),
            json!([{"id":"a".repeat(32),"offset":1,"extra":true}]),
        ] {
            assert!(progress(value).is_err());
        }
        let connection = native::Disconnect::new(None, mpsc::channel(1).0);
        assert!(configure(&connection.0, &json!({"fragmentedMessages":false})).is_err());
        assert!(!connection.0.chunked.load(Ordering::Acquire));
        assert_eq!(
            configure(&connection.0, &json!({"fragmentedMessages":true})).unwrap()["fragmentAcks"],
            true
        );
        assert!(connection.0.chunked.load(Ordering::Acquire));
    }
    #[tokio::test]
    async fn receiving_continues_while_the_other_direction_is_backpressured() {
        use tokio_tungstenite::{WebSocketStream, tungstenite::protocol::Role};
        // Exercise the real WebSocket reader/writer over bounded I/O. Windows
        // loopback TCP can buffer a whole frame despite SO_SNDBUF/SO_RCVBUF.
        let (server, client) = tokio::io::duplex(1024);
        let server = WebSocketStream::from_raw_socket(server, Role::Server, None).await;
        let mut client = WebSocketStream::from_raw_socket(client, Role::Client, None).await;
        let (mut reader, mut writer) = split_relay(server);
        let sending = tokio::spawn(async move {
            writer
                .send(Message::Binary(vec![4; 64 * 1024].into()))
                .await
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            !sending.is_finished(),
            "fixture must actually apply backpressure"
        );
        client
            .send(Message::Binary(b"fixture-key-release".to_vec().into()))
            .await
            .unwrap();
        let message = tokio::time::timeout(Duration::from_secs(1), reader.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(message.into_data().as_ref(), b"fixture-key-release");
        sending.abort();
        let _ = sending.await;
    }
    #[test]
    fn control_passes_a_large_video_and_a_large_response() {
        let mut schedule = Schedule {
            video: Some(Pending::new(vec![7; CHUNK * 3], true)),
            ..Schedule::default()
        };
        assert!(schedule.next().unwrap().len() < dogcodes_remote_transport::PLAINTEXT_CHUNK);
        schedule.control(vec![8; CHUNK * 2], true).unwrap();
        schedule.next().unwrap(); // A large reply preempts the video.
        schedule
            .control(b"input acknowledged".to_vec(), true)
            .unwrap();
        assert_eq!(schedule.next().unwrap(), b"input acknowledged");
        assert!(schedule.video.is_some());
        assert_eq!(schedule.large.len(), 1);
        while schedule.video.is_some() || !schedule.large.is_empty() {
            let progress = schedule
                .flights
                .iter()
                .map(|(id, f)| Progress {
                    id: id.clone(),
                    offset: f.sent,
                })
                .collect();
            schedule.acknowledge(progress).unwrap();
            schedule.next().unwrap();
        }
        assert!(schedule.video.is_none());
    }
    #[test]
    fn legacy_receivers_get_original_frames_and_queues_are_bounded() {
        let bytes = vec![3; CHUNK * 2];
        let mut pending = Pending::new(bytes.clone(), false);
        assert_eq!(pending.next().unwrap(), bytes);
        assert!(pending.done());
        let mut schedule = Schedule::default();
        for _ in 0..LARGE_QUEUE {
            schedule.control(vec![2; CHUNK + 1], true).unwrap();
        }
        assert!(schedule.control(vec![2; CHUNK + 1], true).is_err());
        for _ in 0..SMALL_QUEUE {
            schedule.control(vec![1], true).unwrap();
        }
        assert!(schedule.control(vec![1], true).is_err());
    }
}
