//! TURN over TCP/TLS (RFC 8656). This is TURN framing, not ICE-TCP/RFC 4571.
use crate::runtime::primitives::{Receiver, Sender, channel};
use bytes::BytesMut;
use futures::StreamExt;
use rtc::ice::url::{SchemeType, Url};
use rtc::shared::{FourTuple, TaggedBytesMut, TransportContext, TransportProtocol};
use std::{collections::HashMap, io, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::TcpStream,
};
use tokio_rustls::{
    TlsConnector,
    rustls::{self, pki_types::ServerName},
};

const QUEUE: usize = 256;
const CONNECT_WAIT: Duration = Duration::from_secs(3);
const WRITE_WAIT: Duration = Duration::from_secs(3);

pub(crate) enum Event {
    Packet(TaggedBytesMut),
    Closed(FourTuple),
}
struct Stream {
    tx: Option<tokio::sync::mpsc::Sender<BytesMut>>,
    task: tokio::task::JoinHandle<()>,
    retired: bool,
}
impl Drop for Stream {
    fn drop(&mut self) {
        self.task.abort();
    }
}
pub(crate) struct Streams {
    streams: HashMap<FourTuple, Stream>,
    tx: Sender<Event>,
    pub(crate) rx: Receiver<Event>,
}
impl Default for Streams {
    fn default() -> Self {
        let (tx, rx) = channel(QUEUE);
        Self {
            streams: HashMap::new(),
            tx,
            rx,
        }
    }
}
trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

/// Platform trust plus public roots, no invalid-certificate/name override.
pub(crate) fn tls_config() -> Arc<rustls::ClientConfig> {
    static CONFIG: std::sync::OnceLock<Arc<rustls::ClientConfig>> = std::sync::OnceLock::new();
    CONFIG
        .get_or_init(|| {
            let mut roots = rustls::RootCertStore::empty();
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            for certificate in rustls_native_certs::load_native_certs().certs {
                let _ = roots.add(certificate);
            }
            Arc::new(
                rustls::ClientConfig::builder_with_provider(Arc::new(
                    rustls::crypto::ring::default_provider(),
                ))
                .with_safe_default_protocol_versions()
                .expect("TLS versions")
                .with_root_certificates(roots)
                .with_no_client_auth(),
            )
        })
        .clone()
}

impl Streams {
    pub(crate) fn sibling(&self) -> Self {
        Self {
            streams: HashMap::new(),
            tx: self.tx.clone(),
            rx: self.rx.clone(),
        }
    }
    pub(crate) fn absorb(&mut self, mut other: Self) {
        self.streams.extend(other.streams.drain());
    }
    pub(crate) async fn connect(&mut self, url: &Url) -> io::Result<FourTuple> {
        self.connect_with_tls(url, tls_config()).await
    }
    pub(crate) async fn connect_with_tls(
        &mut self,
        url: &Url,
        config: Arc<rustls::ClientConfig>,
    ) -> io::Result<FourTuple> {
        let (tuple, io): (_, Box<dyn Io>) = tokio::time::timeout(CONNECT_WAIT, async {
            let addresses: Vec<_> = tokio::net::lookup_host((url.host.as_str(), url.port))
                .await?
                .take(16)
                .collect();
            let mut pending: futures::stream::FuturesUnordered<_> = addresses
                .into_iter()
                .enumerate()
                .map(|(index, address)| async move {
                    // Race alternate addresses instead of letting a broken IPv6 route
                    // prevent an otherwise reachable IPv4 TURN server from connecting.
                    if index > 0 {
                        tokio::time::sleep(Duration::from_millis(200)).await;
                    }
                    TcpStream::connect(address).await
                })
                .collect();
            let mut connected = None;
            while let Some(result) = pending.next().await {
                if let Ok(socket) = result {
                    connected = Some(socket);
                    break;
                }
            }
            let socket = connected.ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::ConnectionRefused,
                    "TURN addresses unavailable",
                )
            })?;
            socket.set_nodelay(true)?;
            let tuple = FourTuple {
                local_addr: socket.local_addr()?,
                peer_addr: socket.peer_addr()?,
            };
            let io: Box<dyn Io> = if url.scheme == SchemeType::Turns {
                let name = ServerName::try_from(url.host.clone()).map_err(io::Error::other)?;
                Box::new(TlsConnector::from(config).connect(name, socket).await?)
            } else {
                Box::new(socket)
            };
            Ok::<_, io::Error>((tuple, io))
        })
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "TURN connect timeout"))??;
        let (tx, mut rx) = tokio::sync::mpsc::channel::<BytesMut>(QUEUE);
        let output = self.tx.clone();
        let task = tokio::spawn(async move {
            let (mut reader, mut writer) = tokio::io::split(io);
            let read_output = output.clone();
            let read = async {
                loop {
                    let packet = read_packet(&mut reader).await?;
                    read_output
                        .send(Event::Packet(TaggedBytesMut {
                            now: std::time::Instant::now(),
                            transport: TransportContext {
                                local_addr: tuple.local_addr,
                                peer_addr: tuple.peer_addr,
                                transport_protocol: TransportProtocol::TCP,
                                ecn: None,
                            },
                            message: packet,
                        }))
                        .await
                        .map_err(|_| io::Error::other("TURN receiver closed"))?;
                }
                #[allow(unreachable_code)]
                Ok::<(), io::Error>(())
            };
            let write = async {
                while let Some(packet) = rx.recv().await {
                    tokio::time::timeout(WRITE_WAIT, write_packet(&mut writer, &packet))
                        .await
                        .map_err(|_| {
                            io::Error::new(io::ErrorKind::TimedOut, "TURN write timeout")
                        })??;
                }
                writer.shutdown().await
            };
            tokio::select! { _ = read => {}, _ = write => {} }
            let _ = output.send(Event::Closed(tuple)).await;
        });
        self.streams.insert(
            tuple,
            Stream {
                tx: Some(tx),
                task,
                retired: false,
            },
        );
        Ok(tuple)
    }
    pub(crate) fn is_empty(&self) -> bool {
        self.streams.is_empty()
    }
    pub(crate) fn contains(&self, tuple: FourTuple) -> bool {
        self.streams.contains_key(&tuple)
    }
    pub(crate) fn write(&self, packet: &TaggedBytesMut) -> io::Result<usize> {
        let stream = self
            .streams
            .get(&FourTuple::from(&packet.transport))
            .ok_or_else(|| io::Error::other("TURN stream closed"))?;
        let tx = stream
            .tx
            .as_ref()
            .ok_or_else(|| io::Error::other("TURN stream retired"))?;
        tx.try_send(packet.message.clone()).map_err(|_| {
            io::Error::new(io::ErrorKind::WouldBlock, "TURN send queue unavailable")
        })?;
        Ok(packet.message.len())
    }
    pub(crate) fn remove(&mut self, tuple: FourTuple, release: bool) {
        if release {
            if let Some(stream) = self.streams.get_mut(&tuple) {
                stream.retired = true;
            }
        } else {
            self.streams.remove(&tuple);
        }
    }
    pub(crate) async fn finish_retired(&mut self) {
        let tuples: Vec<_> = self
            .streams
            .iter()
            .filter(|(_, s)| s.retired)
            .map(|(k, _)| *k)
            .collect();
        // Poll-writes already enqueued authenticated Refresh(0). Drain the bounded
        // writer before dropping the socket, with an overall shutdown deadline.
        let mut tasks = Vec::new();
        for tuple in tuples {
            if let Some(mut stream) = self.streams.remove(&tuple) {
                stream.tx.take();
                tasks.push(stream);
            }
        }
        let drain = async {
            for stream in &mut tasks {
                let _ = (&mut stream.task).await;
            }
        };
        let _ = tokio::time::timeout(Duration::from_secs(1), drain).await;
    }
}

/// STUN has a 20-byte header; ChannelData has a 4-byte header plus stream padding.
/// Do not prepend ICE-TCP's two-byte length header to TURN messages.
async fn read_packet(reader: &mut (impl AsyncRead + Unpin)) -> io::Result<BytesMut> {
    let mut head = [0; 4];
    reader.read_exact(&mut head).await?;
    let payload = usize::from(u16::from_be_bytes([head[2], head[3]]));
    let (length, padded) = match head[0] >> 6 {
        0 if payload % 4 == 0 => (20 + payload, 20 + payload),
        1 if head[0] <= 0x7f => (4 + payload, 4 + ((payload + 3) & !3)),
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Invalid TURN stream frame",
            ));
        }
    };
    let mut packet = BytesMut::zeroed(padded);
    packet[..4].copy_from_slice(&head);
    reader.read_exact(&mut packet[4..]).await?;
    if head[0] >> 6 == 0 && packet[4..8] != [0x21, 0x12, 0xa4, 0x42] {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Invalid STUN cookie",
        ));
    }
    packet.truncate(length);
    Ok(packet)
}
async fn write_packet(writer: &mut (impl AsyncWrite + Unpin), packet: &[u8]) -> io::Result<()> {
    writer.write_all(packet).await?;
    if packet.first().is_some_and(|byte| byte >> 6 == 1) {
        let padding = (4 - packet.len() % 4) % 4;
        writer.write_all(&[0; 3][..padding]).await?;
    }
    writer.flush().await
}
