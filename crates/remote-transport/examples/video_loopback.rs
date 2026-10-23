//! Hardware-encoded synthetic screen → screen track → a libwebrtc-like
//! receiver over loopback: reports frame rate, capture-to-receipt latency,
//! bitrate and the congestion controller's target, and writes what arrived
//! as Annex-B for `ffprobe`:
//! `cargo run --release -p dogcodes-remote-transport --example video_loopback -- out.h264`.
use dogcodes_remote_transport::answer_with_video;
use rtc::interceptor::Registry;
use rtc::peer_connection::configuration::interceptor_registry::register_default_interceptors;
use rtc::peer_connection::configuration::media_engine::MediaEngine;
use rtc::rtp::codec::h264::H264Packet;
use rtc::rtp::packetizer::Depacketizer;
use rtc::rtp_transceiver::rtp_sender::RtpCodecKind;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{Notify, mpsc};
use webrtc::media_stream::track_remote::{TrackRemote, TrackRemoteEvent};
use webrtc::peer_connection::{
    PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler, RTCIceGatheringState,
};
use webrtc::rtp_transceiver::{RTCRtpTransceiverDirection, RTCRtpTransceiverInit};

static REFUSED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

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

#[allow(clippy::cast_precision_loss)]
#[tokio::main]
async fn main() {
    let out = std::env::args().nth(1);
    let mut engine = MediaEngine::default();
    engine.register_default_codecs().unwrap();
    let registry = register_default_interceptors(Registry::new(), &mut engine).unwrap();
    let gathered = Arc::new(Notify::new());
    let (tx, mut tracks) = mpsc::channel(1);
    let phone: Arc<dyn PeerConnection> = Arc::new(
        PeerConnectionBuilder::<std::net::SocketAddr>::new()
            .with_udp_addrs(vec![std::net::SocketAddr::from(([127, 0, 0, 1], 0))])
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
    let _channel = phone
        .create_data_channel("dogcodes-v1", None)
        .await
        .unwrap();
    phone
        .set_local_description(phone.create_offer(None).await.unwrap())
        .await
        .unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(3), gathered.notified()).await;
    let offer = phone.local_description().await.unwrap();
    let server = answer_with_video(offer, vec![]).await.unwrap();
    let video = server.video.clone().expect("H.264 negotiated");
    phone
        .set_remote_description(server.description.clone())
        .await
        .unwrap();
    let accept = tokio::spawn(server.accept());

    // Encode on a plain thread at 30 fps, as the capture queue does on the
    // PC, following the congestion controller's target like the daemon.
    let captured: Arc<Mutex<Vec<Instant>>> = Arc::default();
    let encoder = {
        let (video, captured) = (video.clone(), captured.clone());
        std::thread::spawn(move || {
            let sink = {
                let (video, captured) = (video.clone(), captured.clone());
                Arc::new(move |frame: dogcodes_remote_desktop::video::EncodedFrame| {
                    let at = Instant::now()
                        .checked_sub(frame.latency)
                        .unwrap_or_else(Instant::now);
                    if video.push(frame.data, at) {
                        captured.lock().unwrap().push(at);
                    } else {
                        REFUSED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                })
            };
            let mut encoder = dogcodes_remote_desktop::video::SyntheticEncoder::new(
                (1920, 1240),
                30,
                1_700_000,
                sink,
            )
            .expect("encoder");
            let mut targets = Vec::new();
            let start = Instant::now();
            for n in 0..300u32 {
                if n % 30 == 0 {
                    let target = video.target_bitrate();
                    encoder.set_rate(u32::try_from(target * 85 / 100).unwrap_or(u32::MAX), 30);
                    targets.push((n / 30, target));
                }
                encoder
                    .encode_next(video.take_keyframe_request())
                    .expect("encode");
                // Absolute deadlines: oversleeping on a busy machine does
                // not accumulate into a lower frame rate.
                let due = start + Duration::from_millis(33) * (n + 1);
                std::thread::sleep(due.saturating_duration_since(Instant::now()));
            }
            (targets, start.elapsed())
        })
    };
    let track = tokio::time::timeout(Duration::from_secs(10), tracks.recv())
        .await
        .expect("track")
        .unwrap();
    let mut depacketizer = H264Packet::default();
    let mut annexb = Vec::new();
    let mut latencies = Vec::new();
    let mut bytes = 0usize;
    let mut frames = 0usize;
    let started = Instant::now();
    let _ = tokio::time::timeout(Duration::from_secs(13), async {
        while let Some(event) = track.poll().await {
            if let TrackRemoteEvent::OnRtpPacket(packet) = event {
                bytes += packet.payload.len();
                if let Ok(nal) = depacketizer.depacketize(&packet.payload) {
                    annexb.extend_from_slice(&nal);
                }
                if packet.header.marker {
                    if let Some(at) = captured.lock().unwrap().get(frames) {
                        latencies.push(at.elapsed());
                    }
                    frames += 1;
                }
            }
        }
    })
    .await;
    let span = started.elapsed();
    let (targets, encoding) = encoder.join().unwrap();
    latencies.sort();
    let pick = |q: usize| {
        latencies
            .get(latencies.len() * q / 100)
            .copied()
            .unwrap_or_default()
    };
    println!(
        "received {frames} frames in {span:.1?}: {:.1} fps, {:.0} kbit/s; capture→receipt latency p50 {:?} p95 {:?} max {:?}",
        frames as f64 / span.as_secs_f64(),
        bytes as f64 * 8.0 / span.as_secs_f64() / 1000.0,
        pick(50),
        pick(95),
        latencies.last().copied().unwrap_or_default(),
    );
    println!(
        "encoder emitted {} of 300 frames ({} refused by a full send queue)",
        captured.lock().unwrap().len() + REFUSED.load(std::sync::atomic::Ordering::Relaxed),
        REFUSED.load(std::sync::atomic::Ordering::Relaxed)
    );
    println!(
        "encoded for {encoding:.1?}; congestion-control target by second (bit/s): {targets:?}"
    );
    if let Some(path) = out {
        std::fs::write(&path, &annexb).unwrap();
        println!("wrote {path}");
    }
    drop(accept);
    let _ = phone.close().await;
}
