//! Measures capture + encode cost of the frame API on this machine:
//! `cargo run --release -p dogcodes-remote-desktop --example frame_bench`.
use base64::{Engine, engine::general_purpose::STANDARD};
use std::time::Instant;

fn main() {
    let display = dogcodes_remote_desktop::displays().expect("displays");
    let primary = display.iter().find(|d| d.primary).unwrap_or(&display[0]);
    println!(
        "display {} {}x{} pt, scale {}",
        primary.id, primary.width, primary.height, primary.scale
    );
    let monitor = xcap::Monitor::all()
        .expect("monitors")
        .into_iter()
        .find(|m| m.id().is_ok_and(|id| id == primary.id))
        .expect("monitor");
    let start = Instant::now();
    for _ in 0..10 {
        dogcodes_remote_desktop::require_unlocked().expect("unlocked");
    }
    println!("lock check: {:?}", start.elapsed() / 10);
    for _ in 0..3 {
        let start = Instant::now();
        let raw = monitor.capture_image().expect("capture");
        let captured = start.elapsed();
        let scaled = image::DynamicImage::ImageRgba8(raw)
            .thumbnail(1600, 1440)
            .to_rgb8();
        let resized = start.elapsed();
        let mut out = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 60)
            .encode_image(&scaled)
            .expect("encode");
        println!(
            "stages: capture {:?}, scale {:?}, encode {:?}",
            captured,
            resized - captured,
            start.elapsed() - resized
        );
    }
    for (label, reuse) in [("per-call frame()", false), ("reused Capturer", true)] {
        for (width, quality) in [(1600, 60), (1280, 60), (960, 50)] {
            let mut capturer =
                dogcodes_remote_desktop::Capturer::new(primary.id).expect("capturer");
            let (mut total, mut bytes, mut unchanged) = (0u128, 0usize, 0);
            let rounds = 10;
            let mut known: Option<String> = None;
            for _ in 0..rounds {
                let start = Instant::now();
                let frame = if reuse {
                    capturer.frame(width, quality, known.as_deref())
                } else {
                    dogcodes_remote_desktop::frame(primary.id, width, quality)
                }
                .expect("frame");
                total += start.elapsed().as_micros();
                known = frame["frameId"].as_str().map(str::to_owned);
                if frame["unchanged"] == true {
                    unchanged += 1;
                }
                bytes += STANDARD
                    .decode(frame["bytes"].as_str().unwrap_or_default())
                    .map_or(0, |b| b.len());
            }
            println!(
                "{label} maxWidth {width} q{quality}: {:.1} ms/frame, {:.0} KB/frame, {unchanged}/{rounds} unchanged",
                total as f64 / f64::from(rounds) / 1000.0,
                bytes as f64 / f64::from(rounds) / 1024.0
            );
        }
    }
}
