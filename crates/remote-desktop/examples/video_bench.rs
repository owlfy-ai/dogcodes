//! Captures the primary display through the video pipeline for a few
//! seconds and reports frame rate, encode latency and bitrate:
//! `cargo run --release -p dogcodes-remote-desktop --example video_bench -- out.h264`.
use dogcodes_remote_desktop::video::{EncodedFrame, VideoConfig, VideoStream};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

fn main() {
    let out = std::env::args().nth(1);
    if let Some(path) = std::env::args().skip_while(|a| a != "--sample").nth(1) {
        return sample(&path);
    }
    if std::env::args().any(|a| a == "--compare") {
        return compare();
    }
    if std::env::args().any(|a| a == "--synthetic") {
        return synthetic(out.filter(|o| o != "--synthetic"));
    }
    let displays = dogcodes_remote_desktop::displays().expect("displays");
    let primary = displays.iter().find(|d| d.primary).unwrap_or(&displays[0]);
    let frames: Arc<Mutex<Vec<(Instant, EncodedFrame)>>> = Arc::default();
    let sink = frames.clone();
    let started = Instant::now();
    let stream = VideoStream::start(
        VideoConfig {
            display: primary.id,
            max_width: 1920,
            profile: dogcodes_remote_desktop::video::VideoProfile::ConstrainedBaselineAuto,
            fps: 30,
            bitrate: 4_000_000,
        },
        Arc::new(move |frame| sink.lock().unwrap().push((Instant::now(), frame))),
    )
    .expect("video stream");
    println!(
        "started in {:?}, stream {:?}",
        started.elapsed(),
        stream.size()
    );
    std::thread::sleep(Duration::from_secs(3));
    stream.request_keyframe();
    std::thread::sleep(Duration::from_secs(2));
    stream.set_bitrate(1_000_000);
    stream.set_fps(15);
    std::thread::sleep(Duration::from_secs(2));
    drop(stream);
    let frames = frames.lock().unwrap();
    let bytes: usize = frames.iter().map(|(_, f)| f.data.len()).sum();
    let keys = frames.iter().filter(|(_, f)| f.keyframe).count();
    let latency: Duration = frames.iter().map(|(_, f)| f.latency).sum();
    let span = match (frames.first(), frames.last()) {
        (Some(a), Some(b)) => b.0 - a.0,
        _ => Duration::ZERO,
    };
    println!(
        "{} frames ({keys} keyframes) over {span:?}: {:.1} fps, {:.0} kbit/s, encode latency {:?} avg",
        frames.len(),
        frames.len() as f64 / span.as_secs_f64().max(0.001),
        bytes as f64 * 8.0 / span.as_secs_f64().max(0.001) / 1000.0,
        latency / u32::try_from(frames.len().max(1)).unwrap_or(1),
    );
    if let Some(path) = out {
        let all: Vec<u8> = frames.iter().flat_map(|(_, f)| f.data.clone()).collect();
        std::fs::write(&path, all).expect("write");
        println!("wrote {path}");
    }
}

/// Without screen-recording permission: encode a moving test pattern.
fn synthetic(out: Option<String>) {
    {
        let emitted = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = emitted.clone();
        let mut encoder = dogcodes_remote_desktop::video::SyntheticEncoder::new(
            (1920, 1240),
            30,
            2_000_000,
            Arc::new(move |_| {
                counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }),
        )
        .expect("encoder");
        let mut calls = Duration::ZERO;
        for _ in 0..90 {
            let tick = Instant::now();
            encoder.encode_next(false).expect("encode");
            calls += tick.elapsed();
            std::thread::sleep(Duration::from_millis(33).saturating_sub(tick.elapsed()));
        }
        std::thread::sleep(Duration::from_millis(200));
        println!(
            "encode_next avg {:?} (pattern + submit); {} of 90 frames emitted",
            calls / 90,
            emitted.load(std::sync::atomic::Ordering::Relaxed)
        );
    }
    for (bitrate, fps) in [(4_000_000, 30), (1_000_000, 15)] {
        let started = Instant::now();
        let frames = dogcodes_remote_desktop::video::encode_synthetic(
            (1920, 1240),
            fps * 4,
            fps,
            bitrate,
            &[fps * 2],
        )
        .expect("encode");
        let span = started.elapsed();
        let bytes: usize = frames.iter().map(|f| f.data.len()).sum();
        let keys = frames.iter().filter(|f| f.keyframe).count();
        let latency: Duration = frames.iter().map(|f| f.latency).sum();
        let mut sorted: Vec<Duration> = frames.iter().map(|f| f.latency).collect();
        sorted.sort();
        println!(
            "synthetic 1920x1240 @{fps} target {} kbit/s: {} frames ({keys} key), {:.0} kbit/s, encode latency avg {:?} p95 {:?}",
            bitrate / 1000,
            frames.len(),
            bytes as f64 * 8.0 / span.as_secs_f64() / 1000.0,
            latency / u32::try_from(frames.len().max(1)).unwrap_or(1),
            sorted
                .get(sorted.len() * 95 / 100)
                .copied()
                .unwrap_or_default(),
        );
        if let Some(path) = &out {
            let all: Vec<u8> = frames.iter().flat_map(|f| f.data.clone()).collect();
            std::fs::write(format!("{path}.{fps}"), all).expect("write");
        }
    }
}

/// A small real stream for decoder tests: 480×320, 20 frames, an IDR at 0
/// and 12, written as `[u32 length][u8 keyframe][Annex-B]` records.
fn sample(path: &str) {
    let frames =
        dogcodes_remote_desktop::video::encode_synthetic((480, 320), 20, 30, 400_000, &[12])
            .expect("encode");
    let mut out = Vec::new();
    for frame in &frames {
        out.extend(u32::try_from(frame.data.len()).unwrap().to_be_bytes());
        out.push(u8::from(frame.keyframe));
        out.extend(&frame.data);
    }
    std::fs::write(path, &out).expect("write");
    println!("{} frames, {} bytes -> {path}", frames.len(), out.len());
}

/// Same moving desktop pattern as the synthetic encoder, 30 frames at
/// 1920×1240: bytes per second as JPEG pulls (q60, + base64 on the old JSON
/// path) against the H.264 stream at 2 Mbit/s.
fn compare() {
    let (w, h) = (1920usize, 1240usize);
    let mut jpeg_bytes = 0usize;
    for t in 0..30usize {
        let mut luma = vec![0u8; w * h];
        for y in 0..h {
            for x in 0..w {
                let window = (w / 8..w / 2).contains(&x) && (h / 6..h * 2 / 3).contains(&y);
                let text = window
                    && ((y + t * 8) / 12).is_multiple_of(2)
                    && !(x / 5 + (y + t * 8) / 24).is_multiple_of(3);
                let pointer = (t * 24) % w <= x
                    && x < (t * 24) % w + 24
                    && (h * 3 / 4..h * 3 / 4 + 24).contains(&y);
                luma[y * w + x] = if text || pointer {
                    235
                } else if window {
                    40
                } else {
                    16 + ((x ^ y) & 0x3f) as u8
                };
            }
        }
        let image = image::GrayImage::from_raw(w as u32, h as u32, luma).unwrap();
        let rgb = image::DynamicImage::ImageLuma8(image).to_rgb8();
        let mut out = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 60)
            .encode_image(&rgb)
            .unwrap();
        jpeg_bytes += out.len();
    }
    let h264 =
        dogcodes_remote_desktop::video::encode_synthetic((1920, 1240), 30, 30, 2_000_000, &[])
            .expect("encode");
    let h264_bytes: usize = h264.iter().map(|f| f.data.len()).sum();
    println!(
        "1 s of 30 fps 1920x1240 desktop motion: JPEG q60 {} KB ({} KB as base64 JSON), H.264 {} KB ({} frames)",
        jpeg_bytes / 1024,
        jpeg_bytes * 4 / 3 / 1024,
        h264_bytes / 1024,
        h264.len()
    );
}
