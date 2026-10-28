//! Windows Media Foundation backend and opt-in hardware acceptance.
#[cfg(test)]
use super::software::Frame;
use super::{EncodedFrame, Result, VideoProfile, accelerated::Input, mf::Native, worker::Worker};
use std::time::Duration;
#[cfg(test)]
use std::time::Instant;
pub(super) fn hardware_attempt(
    size: (u32, u32),
    rate: (u32, u32),
    profile: VideoProfile,
) -> Result<Worker<Input, EncodedFrame>> {
    #[cfg(test)]
    if std::env::var("DOGCODES_MF_ENCODER_FIXTURE").as_deref() != Ok("synthetic-only") {
        return Err("真实硬件编码验收需显式启用".into());
    }
    Worker::start(
        move || Native::new(size, rate, profile),
        |native, (frame, captured, force, rate): Input| {
            native.encode(&frame, captured, force, rate)
        },
        Duration::from_secs(2),
    )
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires a Windows hardware MFT; synthetic frames only"]
    fn isolated_hardware_encoder_emits_decodable_synthetic_idrs_and_live_rate_changes() {
        assert_eq!(
            std::env::var("DOGCODES_MF_ENCODER_FIXTURE").as_deref(),
            Ok("synthetic-only")
        );
        let mut worker = Worker::start(
            || {
                Native::new(
                    (320, 240),
                    (1_000_000, 30),
                    VideoProfile::ConstrainedBaseline31,
                )
            },
            |codec, (frame, captured, force, rate): Input| {
                codec.encode(&frame, captured, force, rate)
            },
            Duration::from_secs(2),
        )
        .unwrap();
        let mut decoder = openh264::decoder::Decoder::new().unwrap();
        for i in 0..6 {
            let frame = worker
                .run((
                    Frame::synthetic(320, 240, i).unwrap(),
                    Instant::now(),
                    i == 0,
                    ((i + 1) * 400_000, if i < 3 { 30 } else { 20 }),
                ))
                .unwrap();
            super::super::validate_profile(&frame, VideoProfile::ConstrainedBaseline31).unwrap();
            if i == 0 || i == 3 {
                assert!(frame.keyframe);
            }
            use openh264::formats::YUVSource;
            assert_eq!(
                decoder.decode(&frame.data).unwrap().unwrap().dimensions(),
                (320, 240)
            );
        }
    }
}
