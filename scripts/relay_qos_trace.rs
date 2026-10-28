//! A deterministic ACK-controller trace with a synthetic FIFO link. No network,
//! desktop capture or Agent is used. Build outside the checkout:
//! rustc --edition=2024 scripts/relay_qos_trace.rs -o /tmp/dogcodes-qos-trace
//! /tmp/dogcodes-qos-trace
//!
//! The model bounds outstanding frames at eight, assumes 20 fps and payloads
//! at 90% of the target by default. Scenarios cover capacity changes, high path delay,
//! jitter and batched ACKs. `rustc --test` also runs the model regressions.
//! It does not model H.264 IDR bursts, retransmission, transport fragmentation,
//! the real decoder, route migration or the full RelayVideo admission policy.
#[path = "../apps/desktop/src-tauri/src/daemon/remote/video/qos.rs"]
mod qos;
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};
struct Packet {
    sequence: u64,
    sent: Instant,
    left: f64,
    ack: Option<Instant>,
}
#[derive(Clone, Copy)]
struct Scenario<'a> {
    name: &'static str,
    capacity: &'a [(u64, f64)],
    path_ms: u64,
    jitter_ms: u64,
    batch_ms: u64,
    frame_ms: u64,
    payload_ratio: f64,
}
const RECOVERY: Scenario<'static> = Scenario {
    name: "drop-and-recovery",
    capacity: &[(0, 1_500_000.0), (4, 300_000.0), (14, 1_500_000.0)],
    path_ms: 80,
    jitter_ms: 0,
    batch_ms: 1,
    frame_ms: 50,
    payload_ratio: 0.9,
};
const STABLE: Scenario<'static> = Scenario {
    name: "stable",
    capacity: &[(0, 1_500_000.0)],
    ..RECOVERY
};
const SCENARIOS: &[Scenario<'static>] = &[
    RECOVERY,
    STABLE,
    Scenario {
        name: "permanent-drop",
        capacity: &[(0, 1_500_000.0), (4, 300_000.0)],
        ..RECOVERY
    },
    Scenario {
        name: "higher-capacity",
        capacity: &[(0, 1_500_000.0), (14, 3_000_000.0)],
        ..RECOVERY
    },
    Scenario {
        name: "high-path-delay",
        path_ms: 500,
        ..STABLE
    },
    Scenario {
        name: "isolated-ack-jitter",
        jitter_ms: 150,
        ..STABLE
    },
    Scenario {
        name: "batched-acks",
        batch_ms: 200,
        ..STABLE
    },
];
struct Point {
    second: f64,
    capacity: f64,
    target: u64,
    inflight: usize,
    delivered_bits: f64,
    delay_ms: u64,
}
fn run(scenario: Scenario<'_>) -> Vec<Point> {
    let start = Instant::now();
    let mut q = qos::RelayQos::new();
    let mut packets: VecDeque<Packet> = VecDeque::new();
    let mut brake = None;
    let mut sequence = 0;
    let mut points = Vec::new();
    let mut delivered_bits = 0.0;
    let mut delay_ms = 0;
    for step in 0..=6000 {
        let now = start + Duration::from_millis(step * 10);
        let capacity = scenario
            .capacity
            .iter()
            .rev()
            .find(|(s, _)| step >= s * 100)
            .unwrap()
            .1;
        let mut work = capacity * 0.01 / 8.0;
        for p in &mut packets {
            if p.left > 0.0 && work > 0.0 {
                let used = p.left.min(work);
                p.left -= used;
                work -= used;
                delivered_bits += used * 8.0;
                if p.left < 0.001 {
                    p.left = 0.0;
                    // Every twentieth packet gets one deterministic delay spike;
                    // the ordered receiver also holds following cumulative ACKs.
                    let jitter = if p.sequence % 20 == 0 {
                        scenario.jitter_ms
                    } else {
                        0
                    };
                    let due_ms = (step * 10 + scenario.path_ms + jitter)
                        .div_ceil(scenario.batch_ms)
                        * scenario.batch_ms;
                    p.ack = Some(start + Duration::from_millis(due_ms));
                }
            }
        }
        let mut newest = None;
        while packets
            .front()
            .is_some_and(|p| p.ack.is_some_and(|at| at <= now))
        {
            newest = packets.pop_front();
        }
        if let Some(p) = newest {
            // RelayVideo learns from the newest frame of a cumulative ACK,
            // rather than treating each released frame as a fresh sample.
            q.on_ack(p.sent, now);
            delay_ms = now.duration_since(p.sent).as_millis().try_into().unwrap();
            if packets.is_empty() {
                brake = None;
            }
        }
        if step % 10 == 0
            && packets
                .front()
                .is_some_and(|p| now.duration_since(p.sent) >= qos::RelayQos::STALL)
            && brake.is_none_or(|at| now.duration_since(at) >= Duration::from_secs(1))
        {
            q.on_stall(now);
            brake = Some(now);
        }
        if step % (scenario.frame_ms / 10) == 0 && packets.len() < 8 {
            sequence += 1;
            packets.push_back(Packet {
                sequence,
                sent: now,
                left: q.bitrate() as f64 * scenario.payload_ratio / 8.0 * scenario.frame_ms as f64
                    / 1000.0,
                ack: None,
            });
        }
        if step % 10 == 0 {
            points.push(Point {
                second: step as f64 / 100.0,
                capacity,
                target: q.bitrate(),
                inflight: packets.len(),
                delivered_bits,
                delay_ms,
            });
        }
    }
    points
}
fn main() {
    for &scenario in SCENARIOS {
        println!(
            "{}: virtual_s capacity_kbps target_kbps inflight",
            scenario.name
        );
        let points = run(scenario);
        for point in points.iter().step_by(20) {
            println!(
                "{:.0} {:.0} {} {}",
                point.second,
                point.capacity / 1000.0,
                point.target / 1000,
                point.inflight
            );
        }
        let tail: Vec<_> = points
            .iter()
            .filter(|p| p.second >= 40.0)
            .map(|p| p.target)
            .collect();
        println!(
            "tail min={} max={} mean={} delivered_bps={:.0} sampled_p95_ms={}",
            tail.iter().min().unwrap(),
            tail.iter().max().unwrap(),
            tail.iter().sum::<u64>() / tail.len() as u64,
            tail_delivery(&points),
            {
                let mut delays: Vec<_> = points
                    .iter()
                    .filter(|p| p.second >= 40.0)
                    .map(|p| p.delay_ms)
                    .collect();
                delays.sort_unstable();
                delays[delays.len() * 95 / 100]
            }
        );
        if scenario.name == RECOVERY.name {
            for (label, target) in [("90%", 1_350_000), ("100%", 1_500_000)] {
                let point = points
                    .iter()
                    .find(|p| p.second >= 14.0 && p.target >= target);
                println!(
                    "recovery_{label}_seconds={:?}",
                    point.map(|p| p.second - 14.0)
                );
            }
        }
    }
}

fn tail_delivery(points: &[Point]) -> f64 {
    (points.last().unwrap().delivered_bits - points[400].delivered_bits) / 20.0
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn restored_capacity_does_not_require_twenty_seconds_to_recover() {
        let points = run(RECOVERY);
        let restored = points
            .iter()
            .find(|p| p.second >= 14.0 && p.target >= 1_350_000)
            .expect("never recovered");
        assert!(
            restored.second <= 26.0,
            "recovery took {} seconds",
            restored.second - 14.0
        );
    }
    #[test]
    fn stable_link_does_not_repeat_deep_sawtooth() {
        let points = run(STABLE);
        let tail: Vec<_> = points
            .iter()
            .filter(|p| p.second >= 20.0)
            .map(|p| p.target)
            .collect();
        let low = *tail.iter().min().unwrap();
        let high = *tail.iter().max().unwrap();
        assert!(
            high as f64 / (low as f64) < 1.3,
            "deep oscillation: {low}..{high}"
        );
        assert!(low >= 1_200_000, "underused stable link: {low}");
    }

    #[test]
    fn a_capacity_drop_still_brakes_promptly() {
        let points = run(RECOVERY);
        assert!(
            points[60].target <= 600_000,
            "did not brake within two seconds"
        );
        assert!(
            points[80].target <= 400_000,
            "continued filling the slow path"
        );
    }

    #[test]
    fn delay_jitter_and_cumulative_ack_batches_do_not_starve_the_link() {
        for &scenario in SCENARIOS {
            let points = run(scenario);
            let capacity = points.last().unwrap().capacity;
            let delivered = tail_delivery(&points);
            assert!(
                delivered >= capacity * 0.8,
                "{}: delivered {delivered}/{capacity}",
                scenario.name
            );
            assert!(
                points
                    .iter()
                    .all(|p| p.inflight <= 8 && (200_000..=8_000_000).contains(&p.target))
            );
        }
    }

    #[test]
    fn encoder_load_and_frame_cadence_do_not_pin_the_old_capacity() {
        for payload_ratio in [0.6, 1.0] {
            for frame_ms in [20, 100] {
                for capacity in [500_000.0, 4_000_000.0] {
                    let capacities = [(0, capacity)];
                    let points = run(Scenario {
                        name: "load-and-cadence",
                        capacity: &capacities,
                        payload_ratio,
                        frame_ms,
                        ..STABLE
                    });
                    assert!(
                        tail_delivery(&points) >= capacity * 0.75,
                        "load={payload_ratio}, frame_ms={frame_ms}, capacity={capacity}: {}",
                        tail_delivery(&points)
                    );
                }
            }
        }
    }
}
