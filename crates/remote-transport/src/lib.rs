//! Ordered encrypted-message transport over ICE/DTLS/SCTP.
//! Pairing, Noise authentication and business dispatch remain endpoint responsibilities.
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Notify, mpsc};
use webrtc::data_channel::{DataChannel, DataChannelEvent};
pub use webrtc::peer_connection::RTCIceServer as IceServer;
pub use webrtc::peer_connection::RTCSdpType;
pub use webrtc::peer_connection::RTCSessionDescription as Description;
use webrtc::peer_connection::{
    PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler, RTCConfigurationBuilder,
    RTCIceGatheringState, RTCIceTransportPolicy,
};
pub mod video;
pub use video::VideoSender;
pub const MAX_MESSAGE: usize = 16_384;
pub const PLAINTEXT_CHUNK: usize = 16_000;
type Result<T> = std::result::Result<T, String>;
fn failure(error: impl std::fmt::Display) -> String {
    format!("直连协商失败：{error}")
}
struct NegotiationGuard(Option<Arc<dyn PeerConnection>>);
impl Drop for NegotiationGuard {
    fn drop(&mut self) {
        if let Some(peer) = self.0.take()
            && let Ok(runtime) = tokio::runtime::Handle::try_current()
        {
            runtime.spawn(async move {
                let _ = peer.close().await;
            });
        }
    }
}

struct Handler {
    gathered: Arc<Notify>,
    channel: mpsc::Sender<Arc<dyn DataChannel>>,
    claimed: AtomicBool,
}
#[async_trait::async_trait]
impl PeerConnectionEventHandler for Handler {
    async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
        if state == RTCIceGatheringState::Complete {
            self.gathered.notify_one();
        }
    }
    async fn on_data_channel(&self, channel: Arc<dyn DataChannel>) {
        if self.claimed.swap(true, Ordering::AcqRel)
            || self.channel.try_send(channel.clone()).is_err()
        {
            // Handler callbacks run on the driver; queue close outside that driver.
            tokio::spawn(async move {
                let _ = channel.close().await;
            });
        }
    }
}
pub struct Answer {
    pub description: Description,
    /// Present when the offer asked for the screen track and H.264 was
    /// negotiated on it.
    pub video: Option<VideoSender>,
    peer: Arc<dyn PeerConnection>,
    incoming: mpsc::Receiver<Arc<dyn DataChannel>>,
    handed_off: bool,
}
impl Answer {
    pub async fn accept(mut self) -> Result<Channel> {
        let channel = tokio::time::timeout(Duration::from_secs(15), self.incoming.recv())
            .await
            .map_err(|_| "直连未建立，请保留中继连接")?
            .ok_or("直连已关闭")?;
        validate_channel(&channel).await?;
        wait_open(&channel).await?;
        self.handed_off = true;
        Ok(Channel {
            peer: self.peer.clone(),
            channel,
        })
    }
}
impl Drop for Answer {
    fn drop(&mut self) {
        if !self.handed_off {
            let peer = self.peer.clone();
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    let _ = peer.close().await;
                });
            }
        }
    }
}
pub async fn answer(offer: Description, servers: Vec<IceServer>) -> Result<Answer> {
    answer_with_policy(offer, servers, RTCIceTransportPolicy::All, false).await
}
/// Like [`answer`], and also sends an H.264 screen track when the offer has a
/// video section to receive it.
pub async fn answer_with_video(offer: Description, servers: Vec<IceServer>) -> Result<Answer> {
    answer_with_policy(offer, servers, RTCIceTransportPolicy::All, true).await
}
async fn answer_with_policy(
    offer: Description,
    servers: Vec<IceServer>,
    policy: RTCIceTransportPolicy,
    video: bool,
) -> Result<Answer> {
    if offer.sdp.len() > 256 * 1024 || offer.sdp_type != RTCSdpType::Offer {
        return Err("直连 offer 无效".into());
    }
    if servers.len() > 8 || servers.iter().any(|s| s.urls.len() > 8) {
        return Err("ICE 配置过大".into());
    }
    let (tx, rx) = mpsc::channel(1);
    let gathered = Arc::new(Notify::new());
    let video = video && offer.sdp.contains("m=video");
    let offered_sdp = offer.sdp.clone();
    let target = Arc::new(std::sync::atomic::AtomicU64::new(video::START_BITRATE));
    let keyframe = Arc::new(AtomicBool::new(false));
    let mut builder = PeerConnectionBuilder::<std::net::SocketAddr>::new()
        .with_udp_addrs(vec![
            std::net::SocketAddr::from(([0, 0, 0, 0], 0)),
            std::net::SocketAddr::from(([0_u16; 8], 0)),
        ])
        .with_configuration(
            RTCConfigurationBuilder::new()
                .with_ice_servers(servers)
                .with_ice_transport_policy(policy)
                .build(),
        )
        .with_data_channel_send_buffer_limit(256 * 1024)
        .with_handler(Arc::new(Handler {
            gathered: gathered.clone(),
            channel: tx,
            claimed: AtomicBool::new(false),
        }));
    if video {
        let (engine, registry) = video::engine(&target, &keyframe)?;
        builder = builder
            .with_media_engine(engine)
            .with_interceptor_registry(registry);
    }
    let peer: Arc<dyn PeerConnection> = Arc::new(builder.build().await.map_err(failure)?);
    let mut cleanup = NegotiationGuard(Some(peer.clone()));
    let result = async {
        peer.set_remote_description(offer).await.map_err(failure)?;
        let screen = if video {
            let ssrc = random_ssrc();
            let track = video::track(ssrc);
            let sender = peer.add_track(track.clone()).await.map_err(failure)?;
            Some((ssrc, track, sender))
        } else {
            None
        };
        peer.set_local_description(peer.create_answer(None).await.map_err(failure)?)
            .await
            .map_err(failure)?;
        let mut sender = None;
        if let Some((ssrc, track, rtp)) = screen {
            let parameters = rtp.get_parameters().await.map_err(failure)?;
            // No common H.264 variant: the phone falls back to frame pulls.
            if let Some(codec) = parameters.rtp_parameters.codecs.iter().find(|c| {
                c.rtp_codec.mime_type.eq_ignore_ascii_case("video/H264")
                    && video::allows_baseline31(&offered_sdp, c.payload_type)
            }) {
                sender = Some(VideoSender::start(
                    track,
                    ssrc,
                    codec.payload_type,
                    target.clone(),
                    keyframe.clone(),
                ));
            }
        }
        Ok::<_, String>((gathered_description(&peer, &gathered).await?, sender))
    }
    .await;
    match result {
        Ok((description, video)) => {
            cleanup.0 = None;
            Ok(Answer {
                description,
                video,
                peer,
                incoming: rx,
                handed_off: false,
            })
        }
        Err(error) => {
            let _ = peer.close().await;
            Err(error)
        }
    }
}
pub(crate) fn random_ssrc() -> u32 {
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u128(
        std::time::SystemTime::now()
            .elapsed()
            .map_or(0, |d| d.as_nanos()),
    );
    // Any 32 bits of the per-process random state will do for an SSRC.
    #[allow(clippy::cast_possible_truncation)]
    let ssrc = hasher.finish() as u32;
    ssrc.max(1)
}
// Multi-homed hosts can have unreachable interfaces/TURN addresses. Non-trickle
// SDP can carry the candidates collected so far; a slow unused interface must
// not discard an otherwise usable relay/direct candidate at the deadline.
async fn gathered_description(
    peer: &Arc<dyn PeerConnection>,
    gathered: &Notify,
) -> Result<Description> {
    let _ = tokio::time::timeout(Duration::from_secs(8), gathered.notified()).await;
    let description = peer.local_description().await.ok_or("ICE 描述不可用")?;
    if !description.sdp.contains("a=candidate:") {
        return Err("没有可用的 ICE 候选，请使用中继".into());
    }
    Ok(description)
}

async fn validate_channel(channel: &Arc<dyn DataChannel>) -> Result<()> {
    if channel.label().await.map_err(failure)? != "dogcodes-v1"
        || !channel.ordered().await.map_err(failure)?
        || channel
            .max_packet_life_time()
            .await
            .map_err(failure)?
            .is_some()
        || channel.max_retransmits().await.map_err(failure)?.is_some()
    {
        return Err("直连需要 dogcodes-v1 可靠有序通道".into());
    }
    Ok(())
}
async fn wait_open(channel: &Arc<dyn DataChannel>) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match channel.poll().await {
                Some(DataChannelEvent::OnOpen) => return Ok(()),
                Some(
                    DataChannelEvent::OnError
                    | DataChannelEvent::OnClose
                    | DataChannelEvent::OnClosing,
                )
                | None => return Err("直连通道已关闭".into()),
                _ => {}
            }
        }
    })
    .await
    .map_err(|_| "直连通道开启超时")?
}
pub struct Channel {
    peer: Arc<dyn PeerConnection>,
    channel: Arc<dyn DataChannel>,
}
#[derive(serde::Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Route {
    pub mode: &'static str,
    pub local_candidate_type: Option<String>,
    pub remote_candidate_type: Option<String>,
    pub rtt_seconds: Option<f64>,
    pub bytes_sent: Option<String>,
    pub bytes_received: Option<String>,
}

/// Initiating side, also used by isolated PC-to-PC protocol tests.
pub struct Offer {
    pub description: Description,
    peer: Arc<dyn PeerConnection>,
    channel: Arc<dyn DataChannel>,
    handed_off: bool,
}
pub async fn offer(servers: Vec<IceServer>) -> Result<Offer> {
    offer_with_policy(servers, RTCIceTransportPolicy::All).await
}
async fn offer_with_policy(
    servers: Vec<IceServer>,
    policy: RTCIceTransportPolicy,
) -> Result<Offer> {
    if servers.len() > 8 || servers.iter().any(|s| s.urls.len() > 8) {
        return Err("ICE 配置过大".into());
    }
    let (tx, _rx) = mpsc::channel(1);
    let gathered = Arc::new(Notify::new());
    let peer: Arc<dyn PeerConnection> = Arc::new(
        PeerConnectionBuilder::<std::net::SocketAddr>::new()
            .with_udp_addrs(vec![
                std::net::SocketAddr::from(([0, 0, 0, 0], 0)),
                std::net::SocketAddr::from(([0_u16; 8], 0)),
            ])
            .with_configuration(
                RTCConfigurationBuilder::new()
                    .with_ice_servers(servers)
                    .with_ice_transport_policy(policy)
                    .build(),
            )
            .with_data_channel_send_buffer_limit(256 * 1024)
            .with_handler(Arc::new(Handler {
                gathered: gathered.clone(),
                channel: tx,
                claimed: AtomicBool::new(false),
            }))
            .build()
            .await
            .map_err(failure)?,
    );
    let mut cleanup = NegotiationGuard(Some(peer.clone()));
    let result = async {
        let channel = peer
            .create_data_channel("dogcodes-v1", None)
            .await
            .map_err(failure)?;
        peer.set_local_description(peer.create_offer(None).await.map_err(failure)?)
            .await
            .map_err(failure)?;
        let description = gathered_description(&peer, &gathered).await?;
        Ok::<_, String>((description, channel))
    }
    .await;
    match result {
        Ok((description, channel)) => {
            cleanup.0 = None;
            Ok(Offer {
                description,
                peer,
                channel,
                handed_off: false,
            })
        }
        Err(error) => {
            let _ = peer.close().await;
            Err(error)
        }
    }
}
impl Offer {
    pub async fn complete(mut self, answer: Description) -> Result<Channel> {
        self.peer
            .set_remote_description(answer)
            .await
            .map_err(failure)?;
        wait_open(&self.channel).await?;
        self.handed_off = true;
        Ok(Channel {
            peer: self.peer.clone(),
            channel: self.channel.clone(),
        })
    }
}
impl Drop for Offer {
    fn drop(&mut self) {
        if !self.handed_off {
            let peer = self.peer.clone();
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    let _ = peer.close().await;
                });
            }
        }
    }
}
impl Channel {
    /// Reports the selected ICE pair without exposing IPs, credentials or SDP.
    pub async fn route(&self) -> Route {
        use webrtc::peer_connection::{RTCStatsReportEntry as E, StatsSelector};
        let report = self
            .peer
            .get_stats(std::time::Instant::now(), StatsSelector::None)
            .await;
        let pair = report.iter().find_map(|entry| match entry {
            E::Transport(stats) => match report.get(&stats.selected_candidate_pair_id) {
                Some(E::IceCandidatePair(pair)) => Some(pair),
                _ => None,
            },
            _ => None,
        });
        let mut route = Route {
            mode: "unknown",
            local_candidate_type: None,
            remote_candidate_type: None,
            rtt_seconds: None,
            bytes_sent: None,
            bytes_received: None,
        };
        if let Some(pair) = pair {
            route.rtt_seconds = Some(pair.current_round_trip_time);
            route.bytes_sent = Some(pair.bytes_sent.to_string());
            route.bytes_received = Some(pair.bytes_received.to_string());
        }
        if let Some(sctp) = self.peer.sctp().await
            && let Ok(Some(pair)) = sctp
                .transport()
                .ice_transport()
                .get_selected_candidate_pair()
                .await
        {
            route.local_candidate_type = Some(pair.local().typ.to_string());
            route.remote_candidate_type = Some(pair.remote().typ.to_string());
            route.mode = if route.local_candidate_type.as_deref() == Some("relay")
                || route.remote_candidate_type.as_deref() == Some("relay")
            {
                "turn-relay"
            } else {
                "direct"
            };
        }
        route
    }
    pub async fn send(&self, bytes: &[u8]) -> Result<()> {
        if bytes.is_empty() || bytes.len() > MAX_MESSAGE {
            return Err("直连消息大小无效".into());
        }
        tokio::time::timeout(
            Duration::from_secs(5),
            self.channel.send(bytes::BytesMut::from(bytes)),
        )
        .await
        .map_err(|_| "直连发送超时")?
        .map_err(failure)
    }
    pub async fn receive(&self) -> Result<Option<Vec<u8>>> {
        loop {
            match self.channel.poll().await {
                Some(DataChannelEvent::OnMessage(message)) => {
                    if message.is_string
                        || message.data.is_empty()
                        || message.data.len() > MAX_MESSAGE
                    {
                        return Err("直连消息格式无效".into());
                    }
                    return Ok(Some(message.data.to_vec()));
                }
                Some(DataChannelEvent::OnClose | DataChannelEvent::OnClosing) | None => {
                    return Ok(None);
                }
                Some(DataChannelEvent::OnError) => return Err("直连通道异常".into()),
                _ => {}
            }
        }
    }
    pub async fn close(&self) -> Result<()> {
        self.peer.close().await.map_err(failure)
    }
}
impl Drop for Channel {
    fn drop(&mut self) {
        let peer = self.peer.clone();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let _ = peer.close().await;
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn reliable_data_channel_connects_without_a_relay_and_preserves_binary_order() {
        let client = offer(vec![]).await.unwrap();
        let server = answer(client.description.clone(), vec![]).await.unwrap();
        let description = server.description.clone();
        let (client, server) = tokio::join!(client.complete(description), server.accept());
        let (client, server) = (client.unwrap(), server.unwrap());
        for index in 0..20_u8 {
            client.send(&[index; 16000]).await.unwrap();
            assert_eq!(server.receive().await.unwrap().unwrap(), vec![index; 16000]);
        }
        assert!(client.send(&vec![0; MAX_MESSAGE + 1]).await.is_err());
        server.send(b"response").await.unwrap();
        assert_eq!(client.receive().await.unwrap().unwrap(), b"response");
        assert_eq!(client.route().await.mode, "direct");
        server.close().await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_secs(5), client.receive())
                .await
                .unwrap()
                .unwrap()
                .is_none()
        );
    }
}

#[cfg(test)]
mod video_tests {
    use super::*;
    use rtc::interceptor::Registry;
    use rtc::peer_connection::configuration::interceptor_registry::register_default_interceptors;
    use rtc::peer_connection::configuration::media_engine::MediaEngine;
    use rtc::rtcp::payload_feedbacks::picture_loss_indication::PictureLossIndication;
    use rtc::rtp_transceiver::rtp_sender::RtpCodecKind;
    use webrtc::media_stream::track_remote::{TrackRemote, TrackRemoteEvent};
    use webrtc::rtp_transceiver::{RTCRtpTransceiverDirection, RTCRtpTransceiverInit};

    struct Phone {
        gathered: Arc<Notify>,
        tracks: mpsc::Sender<Arc<dyn TrackRemote>>,
    }
    #[async_trait::async_trait]
    impl PeerConnectionEventHandler for Phone {
        async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
            if state == RTCIceGatheringState::Complete {
                self.gathered.notify_one();
            }
        }
        async fn on_track(&self, track: Arc<dyn TrackRemote>) {
            let _ = self.tracks.send(track).await;
        }
    }

    /// Stands in for the phone: libwebrtc's default codecs and feedback,
    /// a receive-only video transceiver plus the business data channel.
    #[tokio::test]
    async fn screen_track_negotiates_h264_and_carries_frames_and_keyframe_requests() {
        screen_offer(None, true).await;
    }
    #[tokio::test]
    async fn baseline_only_receiver_carries_video() {
        screen_offer(Some("42001f"), true).await;
    }
    #[tokio::test]
    async fn lower_level_receiver_uses_image_fallback() {
        screen_offer(Some("42e015"), false).await;
    }

    async fn screen_offer(profile: Option<&str>, expected: bool) {
        screen_offer_with_ice(profile, expected, vec![], false).await;
    }

    pub(super) async fn screen_offer_with_ice(
        profile: Option<&str>,
        expected: bool,
        servers: Vec<IceServer>,
        relay_only: bool,
    ) {
        let mut engine = MediaEngine::default();
        if let Some(profile) = profile {
            engine
                .register_codec(
                    rtc::rtp_transceiver::rtp_sender::RTCRtpCodecParameters {
                        rtp_codec: video::h264(profile),
                        payload_type: 102,
                    },
                    RtpCodecKind::Video,
                )
                .unwrap();
        } else {
            engine.register_default_codecs().unwrap();
        }
        let registry = register_default_interceptors(Registry::new(), &mut engine).unwrap();
        let gathered = Arc::new(Notify::new());
        let (tx, mut tracks) = mpsc::channel(1);
        let phone: Arc<dyn PeerConnection> = Arc::new(
            PeerConnectionBuilder::<std::net::SocketAddr>::new()
                .with_udp_addrs(vec![std::net::SocketAddr::from(([127, 0, 0, 1], 0))])
                .with_configuration(
                    RTCConfigurationBuilder::new()
                        .with_ice_servers(servers.clone())
                        .with_ice_transport_policy(if relay_only {
                            RTCIceTransportPolicy::Relay
                        } else {
                            RTCIceTransportPolicy::All
                        })
                        .build(),
                )
                .with_media_engine(engine)
                .with_interceptor_registry(registry)
                .with_handler(Arc::new(Phone {
                    gathered: gathered.clone(),
                    tracks: tx,
                }))
                .build()
                .await
                .unwrap(),
        );
        phone
            .add_transceiver_from_kind(
                RtpCodecKind::Video,
                Some(RTCRtpTransceiverInit {
                    direction: RTCRtpTransceiverDirection::Recvonly,
                    streams: vec![],
                    send_encodings: vec![],
                }),
            )
            .await
            .unwrap();
        let channel = phone
            .create_data_channel("dogcodes-v1", None)
            .await
            .unwrap();
        phone
            .set_local_description(phone.create_offer(None).await.unwrap())
            .await
            .unwrap();
        let offer = gathered_description(&phone, &gathered).await.unwrap();
        if relay_only {
            assert!(offer.sdp.contains("typ relay"));
            assert!(!offer.sdp.contains("typ host"));
        } else {
            // A data-only answer to the same offer carries no screen.
            assert!(answer(offer.clone(), vec![]).await.unwrap().video.is_none());
        }
        let server = answer_with_policy(
            offer,
            servers,
            if relay_only {
                RTCIceTransportPolicy::Relay
            } else {
                RTCIceTransportPolicy::All
            },
            true,
        )
        .await
        .unwrap();
        if relay_only {
            assert!(server.description.sdp.contains("typ relay"));
            assert!(!server.description.sdp.contains("typ host"));
        }
        assert!(server.description.sdp.contains("H264"));
        if !expected {
            assert!(server.video.is_none());
            server.peer.close().await.unwrap();
            phone.close().await.unwrap();
            return;
        }
        let video = server.video.clone().expect("H.264 negotiated");
        phone
            .set_remote_description(server.description.clone())
            .await
            .unwrap();
        let (opened, accepted) = tokio::join!(wait_open(&channel), server.accept());
        opened.unwrap();
        let server = accepted.unwrap();
        let client = Channel {
            peer: phone.clone(),
            channel: channel.clone(),
        };
        if relay_only {
            assert_eq!(client.route().await.mode, "turn-relay");
            assert_eq!(server.route().await.mode, "turn-relay");
        }

        // An IDR access unit (SPS, PPS, slice) then delta frames, ~1200 B each.
        let frame = |kind: u8, n: u8| {
            let mut f = Vec::new();
            if kind == 5 {
                f.extend_from_slice(&[0, 0, 0, 1, 0x67, 0x42, 0xe0, 0x1f]);
                f.extend_from_slice(&[0, 0, 0, 1, 0x68, 0xce, 0x3c, 0x80]);
            }
            f.extend_from_slice(&[0, 0, 0, 1, 0x60 | kind]);
            f.extend(std::iter::repeat_n(n, 3000));
            f
        };
        let track = tokio::time::timeout(Duration::from_secs(10), async {
            for n in 0..200u8 {
                assert!(video.push(
                    frame(if n % 50 == 0 { 5 } else { 1 }, n),
                    std::time::Instant::now()
                ));
                tokio::time::sleep(Duration::from_millis(10)).await;
                if let Ok(track) = tracks.try_recv() {
                    return track;
                }
            }
            panic!("no remote track");
        })
        .await
        .unwrap();
        // Bootstrap the remote track before asserting delivery of a specific
        // RTP marker. SCTP opening is not proof that initial media was seen.
        // Direct tests use a current-thread runtime, so the writer cannot
        // poll these queued old units before reset. TURN tests give both
        // devices separate workers; there, already-written old units would
        // legitimately be outside the application's reset boundary.
        if !relay_only {
            assert!(video.push(frame(1, 251), std::time::Instant::now()));
            assert!(video.push(frame(1, 252), std::time::Instant::now()));
        }
        video.reset();
        assert!(video.take_keyframe_request());
        assert!(video.push(frame(5, 253), std::time::Instant::now()));
        let sender = tokio::spawn({
            let video = video.clone();
            async move {
                for n in 0..120u8 {
                    let _ = video.push(frame(1, n), std::time::Instant::now());
                    tokio::time::sleep(Duration::from_millis(15)).await;
                }
            }
        });
        let mut packets = 0;
        let mut recovered = false;
        let mut payload_type = None;
        let media = async {
            while packets < 100 {
                if let Some(TrackRemoteEvent::OnRtpPacket(p)) = track.poll().await {
                    if !relay_only {
                        assert!(!matches!(p.payload.last(), Some(251 | 252)));
                    }
                    recovered |= p.payload.last() == Some(&253);
                    packets += 1;
                    payload_type = Some(p.header.payload_type);
                }
            }
        };
        let control = async {
            client.send(b"control-during-video").await.unwrap();
            assert_eq!(
                server.receive().await.unwrap().unwrap(),
                b"control-during-video"
            );
            server.send(b"video-control-reply").await.unwrap();
            assert_eq!(
                client.receive().await.unwrap().unwrap(),
                b"video-control-reply"
            );
        };
        tokio::time::timeout(Duration::from_secs(10), async {
            tokio::join!(media, control);
        })
        .await
        .expect("RTP packets");
        let codec = track.codec(track.ssrcs().await[0]).await.unwrap();
        assert_eq!(codec.mime_type.to_lowercase(), "video/h264");
        assert!(payload_type.is_some());
        assert!(recovered, "new-generation IDR reached the actual RTP peer");

        assert!(!video.take_keyframe_request());
        track
            .write_rtcp(vec![Box::new(PictureLossIndication {
                sender_ssrc: 1,
                media_ssrc: track.ssrcs().await[0],
            })])
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while !video.take_keyframe_request() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("PLI surfaces as a keyframe request");
        sender.await.unwrap();
        // TWCC feedback reached the estimator: it reports a usable target.
        let target = video.target_bitrate();
        assert!((video::MIN_BITRATE..=video::MAX_BITRATE).contains(&target));
        phone.close().await.unwrap();
    }
}

#[cfg(test)]
mod turn_acceptance {
    use super::*;
    use dogcodes_endpoint::Session;
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "scripts/turn_acceptance.py supplies an isolated authenticated coturn fixture"]
    async fn turn_video_acceptance() {
        let encoded =
            std::fs::read(std::env::var_os("DOGCODES_TURN_TEST_CONFIG").expect("test config path"))
                .unwrap();
        let setup: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
        assert_ne!(setup["reject"], true);
        let ice: IceServer = serde_json::from_value(setup["ice"].clone()).unwrap();
        super::video_tests::screen_offer_with_ice(None, true, vec![ice], true).await;
    }
    // The fixture runs both devices here. Each needs independent reactor
    // progress while a peer flushes a burst, as on separate physical hosts.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "scripts/turn_acceptance.py supplies an isolated authenticated coturn fixture"]
    async fn turn_acceptance() {
        let encoded =
            std::fs::read(std::env::var_os("DOGCODES_TURN_TEST_CONFIG").expect("test config path"))
                .unwrap();
        let setup: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
        let ice: IceServer = serde_json::from_value(setup["ice"].clone()).unwrap();
        let result = offer_with_policy(vec![ice.clone()], RTCIceTransportPolicy::Relay).await;
        if setup["reject"] == true {
            if let Ok(client) = result {
                assert!(!client.description.sdp.contains("typ relay"));
            }
            return;
        }
        let client = result.unwrap();
        assert!(client.description.sdp.contains("typ relay"));
        assert!(!client.description.sdp.contains("typ host"));
        let server = answer_with_policy(
            client.description.clone(),
            vec![ice],
            RTCIceTransportPolicy::Relay,
            false,
        )
        .await
        .unwrap();
        assert!(server.description.sdp.contains("typ relay"));
        let description = server.description.clone();
        let (client, server) = tokio::join!(client.complete(description), server.accept());
        let (client, server) = (client.unwrap(), server.unwrap());
        let phone = dogcodes_endpoint::keypair().unwrap();
        let pc = dogcodes_endpoint::keypair().unwrap();
        let mut a = Session::new(
            true,
            &phone.private.clone().try_into().unwrap(),
            Some(pc.public.clone().try_into().unwrap()),
            None,
            b"turn-acceptance-v1",
        )
        .unwrap();
        let mut b = Session::new(
            false,
            &pc.private.clone().try_into().unwrap(),
            Some(phone.public.clone().try_into().unwrap()),
            None,
            b"turn-acceptance-v1",
        )
        .unwrap();
        client.send(&a.write_handshake().unwrap()).await.unwrap();
        b.read_handshake(&server.receive().await.unwrap().unwrap())
            .unwrap();
        server.send(&b.write_handshake().unwrap()).await.unwrap();
        a.read_handshake(&client.receive().await.unwrap().unwrap())
            .unwrap();
        client.send(&a.write_handshake().unwrap()).await.unwrap();
        b.read_handshake(&server.receive().await.unwrap().unwrap())
            .unwrap();
        assert_eq!(a.binding().unwrap(), b.binding().unwrap());
        let (mut transmitted_messages, mut delivered_messages) = (0, 0);
        let send = async {
            for index in 0..128_u8 {
                client
                    .send(&a.seal(&vec![index; 16000]).unwrap())
                    .await
                    .unwrap();
                transmitted_messages += 1;
            }
        };
        let read = async {
            for index in 0..128_u8 {
                assert_eq!(
                    b.open(&server.receive().await.unwrap().unwrap()).unwrap(),
                    vec![index; 16000]
                );
                delivered_messages += 1;
            }
        };
        let transfer = tokio::time::timeout(Duration::from_secs(20), async {
            tokio::join!(send, read);
        })
        .await;
        assert!(
            transfer.is_ok(),
            "TURN 2 MiB transfer timed out: sent={transmitted_messages}/128 received={delivered_messages}/128"
        );
        server.send(&b.seal(b"receipt").unwrap()).await.unwrap();
        assert_eq!(
            a.open(&client.receive().await.unwrap().unwrap()).unwrap(),
            b"receipt"
        );
        assert_eq!(client.route().await.mode, "turn-relay");
        assert_eq!(server.route().await.mode, "turn-relay");
        client.close().await.unwrap();
        server.close().await.unwrap();
    }
}
