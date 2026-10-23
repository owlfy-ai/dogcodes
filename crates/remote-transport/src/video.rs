//! An H.264 screen track on a direct connection, with send-side congestion
//! control: TWCC feedback drives Google Congestion Control, a pacer releases
//! packets at the estimate, NACKs are answered from the send history, and
//! PLI/FIR feedback is surfaced as a keyframe request. The application reads
//! the estimate and sets its encoder bitrate from it, which is what the
//! `RustDesk` video `QoS` approximates from round-trip probes.
use rtc::interceptor::{
    BandwidthEstimator, CongestionControlBuilder, EstimatorStats, Gcc, Interceptor, PacerBuilder,
    Packet, PacketReport, Registry, Slot, StreamInfo, TaggedPacket,
};
use rtc::media_stream::MediaStreamTrack;
use rtc::peer_connection::configuration::interceptor_registry::{
    configure_nack, configure_rtcp_reports, configure_twcc,
};
use rtc::peer_connection::configuration::media_engine::MediaEngine;
use rtc::rtcp::payload_feedbacks::{
    full_intra_request::FullIntraRequest, picture_loss_indication::PictureLossIndication,
};
use rtc::rtp::codec::h264::H264Payloader;
use rtc::rtp::packetizer::Payloader;
use rtc::rtp_transceiver::rtp_sender::{
    RTCPFeedback, RTCRtpCodec, RTCRtpCodecParameters, RTCRtpCodingParameters,
    RTCRtpEncodingParameters, RtpCodecKind,
};
use std::collections::VecDeque;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use webrtc::media_stream::track_local::TrackLocal;
use webrtc::media_stream::track_local::static_rtp::TrackLocalStaticRTP;

pub const MIN_BITRATE: u64 = 250_000;
pub const START_BITRATE: u64 = 2_000_000;
pub const MAX_BITRATE: u64 = 12_000_000;
const QUEUE_FRAMES: usize = 8;
const PACING_FACTOR: f64 = 2.5;
/// Same outbound MTU as webrtc's own sample packetizer.
const RTP_MTU: usize = 1200;

/// Advertise only profiles the encoder actually produces, at level 3.1.
/// Level asymmetry never permits exceeding the receiver's level (RFC 6184 §8.2.2).
const H264_PROFILES: [(&str, u8); 2] = [("42e01f", 102), ("42001f", 104)];

/// Inspect the original offer, not a rewritten negotiated codec. Some stacks
/// match profiles while ignoring levels. Lower/unsupported receivers use JPEG.
pub(crate) fn allows_baseline31(sdp: &str, payload: u8) -> bool {
    let prefix = format!("a=fmtp:{payload} ");
    let mut video = false;
    for line in sdp.lines().map(str::trim) {
        if line.starts_with("m=") {
            video = line.starts_with("m=video ");
        }
        if !video {
            continue;
        }
        let Some(fmtp) = line.strip_prefix(&prefix) else {
            continue;
        };
        let mut mode = None;
        let mut profile = None;
        for part in fmtp.split(';') {
            let Some((key, value)) = part.trim().split_once('=') else {
                continue;
            };
            match key.trim() {
                "packetization-mode" if mode.replace(value.trim()).is_some() => return false,
                "profile-level-id" if profile.replace(value.trim()).is_some() => return false,
                _ => {}
            }
        }
        if mode != Some("1") {
            return false;
        }
        // Require an explicit receiver budget rather than assuming level 3.1.
        let Some(profile) = profile else {
            return false;
        };
        if profile.len() != 6 || !profile.is_ascii() {
            return false;
        }
        let Ok(bits) = u32::from_str_radix(profile, 16) else {
            return false;
        };
        let level = bits & 0xff;
        return bits >> 16 == 0x42
            && (bits >> 8).trailing_zeros() >= 4
            && matches!(level, 31 | 32 | 40 | 41 | 42 | 50 | 51 | 52 | 60 | 61 | 62);
    }
    false
}

pub(crate) fn h264(profile: &str) -> RTCRtpCodec {
    RTCRtpCodec {
        mime_type: "video/H264".to_owned(),
        clock_rate: 90_000,
        channels: 0,
        sdp_fmtp_line: format!(
            "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id={profile}"
        ),
        rtcp_feedback: vec![
            RTCPFeedback {
                typ: "nack".to_owned(),
                parameter: String::new(),
            },
            RTCPFeedback {
                typ: "nack".to_owned(),
                parameter: "pli".to_owned(),
            },
            RTCPFeedback {
                typ: "ccm".to_owned(),
                parameter: "fir".to_owned(),
            },
        ],
    }
}

/// Gcc behind a seam that also publishes its estimate for the encoder.
struct PublishedGcc {
    inner: Gcc,
    target: Arc<AtomicU64>,
}
impl PublishedGcc {
    fn publish(&self) {
        // Truncation to whole bits per second is intended.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let bits = self.inner.target_bitrate().max(0.0) as u64;
        self.target.store(bits, Ordering::Release);
    }
}
impl BandwidthEstimator for PublishedGcc {
    fn on_reports(&mut self, now: Instant, reports: &[PacketReport]) {
        self.inner.on_reports(now, reports);
        self.publish();
    }
    /// What the pacer releases at: a multiple of the encoder target, as
    /// libwebrtc does, so a keyframe or a burst of change leaves at once
    /// instead of queueing behind the average rate.
    fn target_bitrate(&self) -> f64 {
        self.inner.target_bitrate() * PACING_FACTOR
    }
    fn handle_timeout(&mut self, now: Instant) {
        self.inner.handle_timeout(now);
        self.publish();
    }
    fn poll_timeout(&self) -> Option<Instant> {
        self.inner.poll_timeout()
    }
    fn stats(&self) -> EstimatorStats {
        self.inner.stats()
    }
}

/// Watches inbound RTCP for picture-loss and full-intra requests. The
/// packets themselves still end at the chain's terminus.
struct KeyframeRequests {
    flag: Arc<AtomicBool>,
    read: VecDeque<TaggedPacket>,
    write: VecDeque<TaggedPacket>,
}
impl rtc::sansio::Protocol<TaggedPacket, TaggedPacket, ()> for KeyframeRequests {
    type Rout = TaggedPacket;
    type Wout = TaggedPacket;
    type Eout = ();
    type Error = rtc::shared::error::Error;
    type Time = Instant;
    fn handle_read(&mut self, msg: TaggedPacket) -> Result<(), Self::Error> {
        if let Packet::Rtcp(packets) = &msg.message.packet
            && packets.iter().any(|p| {
                p.as_any().is::<PictureLossIndication>() || p.as_any().is::<FullIntraRequest>()
            })
        {
            self.flag.store(true, Ordering::Release);
        }
        self.read.push_back(msg);
        Ok(())
    }
    fn poll_read(&mut self) -> Option<Self::Rout> {
        self.read.pop_front()
    }
    fn handle_write(&mut self, msg: TaggedPacket) -> Result<(), Self::Error> {
        self.write.push_back(msg);
        Ok(())
    }
    fn poll_write(&mut self) -> Option<Self::Wout> {
        self.write.pop_front()
    }
}
impl Interceptor for KeyframeRequests {
    fn bind_local_stream(&mut self, _: &StreamInfo) {}
    fn unbind_local_stream(&mut self, _: &StreamInfo) {}
    fn bind_remote_stream(&mut self, _: &StreamInfo) {}
    fn unbind_remote_stream(&mut self, _: &StreamInfo) {}
}

/// Media engine (H.264 only) and interceptor chain for a screen sender.
pub(crate) fn engine(
    target: &Arc<AtomicU64>,
    keyframe: &Arc<AtomicBool>,
) -> Result<(MediaEngine, Registry), String> {
    let failure = |e: rtc::shared::error::Error| format!("视频协商失败：{e}");
    let mut engine = MediaEngine::default();
    for (profile, payload_type) in H264_PROFILES {
        engine
            .register_codec(
                RTCRtpCodecParameters {
                    rtp_codec: h264(profile),
                    payload_type,
                },
                RtpCodecKind::Video,
            )
            .map_err(failure)?;
    }
    let registry = configure_nack(Registry::new(), &mut engine);
    let registry = configure_rtcp_reports(registry);
    // `configure_congestion_control`, except that the pacer starts at the
    // paced start rate rather than its 1 Mb/s default: until the first
    // feedback the first keyframe would otherwise trickle out.
    let registry = configure_twcc(registry, &mut engine).map_err(failure)?;
    #[allow(clippy::cast_precision_loss)]
    let registry = registry
        .with(
            Slot::CongestionControl,
            CongestionControlBuilder::new(PublishedGcc {
                inner: Gcc::new(START_BITRATE as f64, MIN_BITRATE as f64, MAX_BITRATE as f64),
                target: target.clone(),
            })
            .build(),
        )
        .with(
            Slot::Pacer,
            PacerBuilder::new()
                .with_target_bitrate(START_BITRATE as f64 * PACING_FACTOR)
                .build(),
        );
    // Application-most, so it sees inbound RTCP after every interceptor.
    let registry = registry.with(
        Slot::from(14_000),
        KeyframeRequests {
            flag: keyframe.clone(),
            read: VecDeque::new(),
            write: VecDeque::new(),
        },
    );
    Ok((engine, registry))
}

pub(crate) fn track(ssrc: u32) -> Arc<TrackLocalStaticRTP> {
    Arc::new(TrackLocalStaticRTP::new(MediaStreamTrack::new(
        "dogcodes-screen".to_owned(),
        "screen".to_owned(),
        "screen".to_owned(),
        RtpCodecKind::Video,
        vec![RTCRtpEncodingParameters {
            rtp_coding_parameters: RTCRtpCodingParameters {
                ssrc: Some(ssrc),
                ..Default::default()
            },
            codec: h264(H264_PROFILES[0].0),
            ..Default::default()
        }],
    )))
}

/// RTP timestamp (90 kHz) for a frame captured `elapsed` after the stream
/// began: from the capture clock, not from sample durations, because screen
/// frames arrive only when something changed — a frame after five idle
/// seconds must carry five seconds more, or the receiver's jitter buffer
/// misreads the gap. Strictly increasing per frame.
pub(crate) fn rtp_timestamp(base: u32, elapsed: Duration, last: Option<u32>) -> u32 {
    let ticks = u32::try_from(elapsed.as_micros() * 9 / 100 % (1u128 << 32)).unwrap_or(0);
    let ts = base.wrapping_add(ticks);
    match last {
        // Wrapping comparison: `ts` not after `last`.
        Some(last) if ts.wrapping_sub(last).wrapping_sub(1) >= 1 << 31 => last.wrapping_add(1),
        _ => ts,
    }
}

struct Frame {
    data: bytes::Bytes,
    captured: Instant,
}

/// Hands encoded frames to the track. Cheap to clone; frames are packetized
/// and written in order by one task, and a full queue refuses rather than
/// blocks so the encoder thread never waits on the network.
#[derive(Clone)]
pub struct VideoSender {
    queue: mpsc::Sender<Frame>,
    target: Arc<AtomicU64>,
    keyframe: Arc<AtomicBool>,
    failed: Arc<AtomicBool>,
}

impl VideoSender {
    pub(crate) fn start(
        track: Arc<TrackLocalStaticRTP>,
        ssrc: u32,
        payload_type: u8,
        target: Arc<AtomicU64>,
        keyframe: Arc<AtomicBool>,
    ) -> Self {
        let (queue, mut frames) = mpsc::channel::<Frame>(QUEUE_FRAMES);
        let failed = Arc::new(AtomicBool::new(false));
        let writer_failed = failed.clone();
        tokio::spawn(async move {
            let mut payloader = H264Payloader::default();
            let started = Instant::now();
            // Random starting points, as RFC 3550 asks.
            let seed = super::random_ssrc();
            let base = seed.rotate_left(16);
            #[allow(clippy::cast_possible_truncation)]
            let mut sequence = seed as u16;
            let mut last = None;
            let mut errors = 0u32;
            'frames: while let Some(frame) = frames.recv().await {
                let timestamp = rtp_timestamp(
                    base,
                    frame.captured.saturating_duration_since(started),
                    last,
                );
                last = Some(timestamp);
                let Ok(payloads) = payloader.payload(RTP_MTU - 12, &frame.data) else {
                    continue;
                };
                let count = payloads.len();
                for (index, payload) in payloads.into_iter().enumerate() {
                    let packet = rtc::rtp::Packet {
                        header: rtc::rtp::Header {
                            version: 2,
                            marker: index + 1 == count,
                            payload_type,
                            sequence_number: sequence,
                            timestamp,
                            ssrc,
                            ..Default::default()
                        },
                        payload,
                    };
                    sequence = sequence.wrapping_add(1);
                    // Writes before negotiation completes fail; a long run of
                    // failures means the connection is gone.
                    if track.write_rtp(packet).await.is_ok() {
                        errors = 0;
                    } else {
                        errors += 1;
                        if errors > 1000 {
                            writer_failed.store(true, Ordering::Release);
                            break 'frames;
                        }
                    }
                }
            }
        });
        Self {
            queue,
            target,
            keyframe,
            failed,
        }
    }

    /// Queues one Annex-B access unit captured at `captured`. `false` when
    /// it was dropped: the caller should request a keyframe, as later frames
    /// reference it.
    #[must_use]
    pub fn push(&self, data: Vec<u8>, captured: Instant) -> bool {
        self.queue
            .try_send(Frame {
                data: data.into(),
                captured,
            })
            .is_ok()
    }

    /// Congestion controller's current target, bits per second.
    #[must_use]
    pub fn target_bitrate(&self) -> u64 {
        self.target
            .load(Ordering::Acquire)
            .clamp(MIN_BITRATE, MAX_BITRATE)
    }

    /// Whether the receiver asked for a keyframe since the last call.
    #[must_use]
    pub fn take_keyframe_request(&self) -> bool {
        self.keyframe.swap(false, Ordering::AcqRel)
    }

    #[must_use]
    pub fn closed(&self) -> bool {
        self.failed.load(Ordering::Acquire) || self.queue.is_closed()
    }
}

#[cfg(test)]
mod tests {
    use super::rtp_timestamp;
    use std::time::Duration;

    #[test]
    fn video_requires_an_explicit_compatible_receiver_budget() {
        let sdp = |profile: &str| {
            format!(
                "m=audio 9 UDP/TLS/RTP/SAVPF 102\r\na=fmtp:102 profile-level-id=64002a;packetization-mode=1\r\nm=video 9 UDP/TLS/RTP/SAVPF 102\r\na=fmtp:102 packetization-mode=1;profile-level-id={profile};level-asymmetry-allowed=1\r\n"
            )
        };
        for profile in ["42e01f", "42001f", "42c020", "42E02A"] {
            assert!(super::allows_baseline31(&sdp(profile), 102));
        }
        for profile in [
            "42e015", "42e00b", "4d001f", "640c1f", "garbage", "42e11f", "42e000",
        ] {
            assert!(!super::allows_baseline31(&sdp(profile), 102));
        }
        assert!(!super::allows_baseline31(&sdp("42e01f"), 104));
        assert!(!super::allows_baseline31(
            &sdp("42e01f").replace("packetization-mode=1", "packetization-mode=0"),
            102
        ));
        assert!(!super::allows_baseline31(
            &sdp("42e01f").replace("profile-level-id=42e01f;", ""),
            102
        ));
    }

    #[test]
    fn timestamps_follow_capture_time_and_never_repeat() {
        let ms = Duration::from_millis;
        assert_eq!(rtp_timestamp(1000, ms(0), None), 1000);
        // 33 ms at 90 kHz.
        assert_eq!(rtp_timestamp(1000, ms(33), Some(1000)), 1000 + 2970);
        // Five idle seconds show up as five seconds.
        assert_eq!(rtp_timestamp(1000, ms(5033), Some(3970)), 1000 + 452_970);
        // Two frames in one tick still get distinct, increasing stamps.
        assert_eq!(rtp_timestamp(1000, ms(33), Some(3970)), 3971);
        // Wrap-around keeps increasing modulo 2^32.
        let near = u32::MAX - 100;
        assert_eq!(
            rtp_timestamp(near, ms(2), Some(near)),
            near.wrapping_add(180)
        );
        assert!(near.wrapping_add(180) < 100);
        assert_eq!(
            rtp_timestamp(near, ms(0), Some(near.wrapping_add(5))),
            near.wrapping_add(6)
        );
    }
}
