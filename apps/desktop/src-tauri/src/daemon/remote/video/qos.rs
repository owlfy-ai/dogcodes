//! Relay ACK learning, independent of unanswered-frame timeout brakes.
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

pub(super) struct RelayQos {
    bitrate: f64,
    samples: VecDeque<u32>,
    baseline: Option<u32>,
    floor_raised: Option<Instant>,
    good: u32,
    bad: u32,
    clean_since: Option<Instant>,
    clean_peak: u32,
    last_cut: Option<Instant>,
    last_rise: Option<Instant>,
    ignore_before: Option<Instant>,
    // Fast recovery stops at a rate actually observed clean, and before the
    // most recent failed level. Small probes must validate that level anew.
    proven: f64,
    congested: Option<f64>,
}
impl RelayQos {
    pub(super) const MIN: f64 = 200_000.0;
    pub(super) const MAX: f64 = 8_000_000.0;
    pub(super) const START: f64 = 1_500_000.0;
    pub(super) const STALL: Duration = Duration::from_secs(2);
    const SAMPLES: usize = 32;

    pub(super) fn new() -> Self {
        Self {
            bitrate: Self::START,
            samples: VecDeque::new(),
            baseline: None,
            floor_raised: None,
            good: 0,
            bad: 0,
            clean_since: None,
            clean_peak: 0,
            last_cut: None,
            last_rise: None,
            ignore_before: None,
            proven: Self::MIN,
            congested: None,
        }
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub(super) fn bitrate(&self) -> u64 {
        self.bitrate as u64
    }

    fn cut(&mut self, factor: f64, now: Instant) {
        self.good = 0;
        self.bad = 0;
        self.clean_since = None;
        self.clean_peak = 0;
        if self
            .last_cut
            .is_some_and(|at| now.duration_since(at) < Duration::from_millis(500))
        {
            return;
        }
        self.last_cut = Some(now);
        self.congested = Some(self.bitrate);
        self.bitrate = (self.bitrate * factor).max(Self::MIN);
    }

    pub(super) fn on_stall(&mut self, now: Instant) {
        // Unanswered frames are not RTT samples. Their eventual ACKs must not
        // trigger a second cut, teach the floor or count as clean recovery.
        self.ignore_before = Some(now);
        self.cut(0.5, now);
    }

    pub(super) fn on_ack(&mut self, sent: Instant, now: Instant) {
        if self.ignore_before.is_some_and(|before| sent <= before) {
            self.good = 0;
            self.bad = 0;
            self.clean_since = None;
            self.clean_peak = 0;
            return;
        }
        let delay = now.duration_since(sent);
        if delay >= Self::STALL {
            self.on_stall(now);
            return;
        }
        // Queued ACKs from the previous target can still report congestion, but
        // cannot establish that the latest increase has survived the path.
        let current =
            self.last_rise.is_none_or(|at| sent >= at) && self.last_cut.is_none_or(|at| sent >= at);
        self.observe(delay, now, current);
    }

    fn floor(&mut self, delay: u32, now: Instant) -> u32 {
        if self.samples.len() == Self::SAMPLES {
            self.samples.pop_front();
        }
        self.samples.push_back(delay);
        let base = self.baseline.unwrap_or(delay).min(delay);
        self.baseline = Some(base);
        if self.samples.len() < Self::SAMPLES
            || self
                .floor_raised
                .is_some_and(|at| now.duration_since(at) < Duration::from_secs(1))
        {
            return base;
        }
        let older = self
            .samples
            .iter()
            .take(Self::SAMPLES / 2)
            .copied()
            .min()
            .unwrap_or(delay);
        let recent = self
            .samples
            .iter()
            .skip(Self::SAMPLES / 2)
            .copied()
            .min()
            .unwrap_or(delay);
        // A rising recent floor is evidence of a queue, not a new path. Once
        // stable, age the old minimum gradually and at most once per second.
        if recent > older.saturating_add(5) {
            return base;
        }
        let next = older.min(recent).min(base.saturating_add(25)).max(base);
        if next > base {
            self.floor_raised = Some(now);
            self.baseline = Some(next);
        }
        next
    }

    #[cfg(test)]
    pub(super) fn on_delay(&mut self, delay: Duration, now: Instant) {
        self.observe(delay, now, true);
    }

    fn observe(&mut self, delay: Duration, now: Instant, current: bool) {
        let delay = u32::try_from(delay.as_millis()).unwrap_or(u32::MAX);
        let excess = delay.saturating_sub(self.floor(delay, now));
        if excess >= 1000 {
            self.cut(0.5, now);
        } else if excess >= 100 {
            self.good = 0;
            self.clean_since = None;
            self.clean_peak = 0;
            self.bad = (self.bad + 1).min(3);
            if self.bad >= 3 {
                self.cut(if excess >= 300 { 0.75 } else { 0.9 }, now);
            }
        } else if excess >= 50 || !current {
            // Approaching the queue target is a hold, not evidence of spare
            // capacity. In particular, do not grow into an existing queue.
            self.good = 0;
            self.bad = 0;
            self.clean_since = None;
            self.clean_peak = 0;
        } else {
            self.bad = 0;
            self.good = (self.good + 1).min(10);
            self.clean_peak = self.clean_peak.max(excess);
            let since = *self.clean_since.get_or_insert(now);
            if self.good >= 10
                && now.duration_since(since) >= Duration::from_secs(1)
                && self
                    .last_cut
                    .is_none_or(|at| now.duration_since(at) >= Duration::from_secs(1))
                && self
                    .last_rise
                    .is_none_or(|at| now.duration_since(at) >= Duration::from_secs(1))
            {
                let low_queue = self.clean_peak < 25;
                self.good = 0;
                self.clean_since = None;
                self.clean_peak = 0;
                self.last_rise = Some(now);
                if low_queue {
                    self.proven = self.proven.max(self.bitrate);
                    if self.congested.is_some_and(|rate| self.bitrate >= rate) {
                        self.congested = None;
                    }
                }
                let ceiling = self
                    .proven
                    .min(self.congested.map_or(Self::MAX, |rate| rate * 0.9));
                self.bitrate = if low_queue && self.bitrate < ceiling {
                    (self.bitrate * 1.25).min(ceiling)
                } else {
                    self.bitrate * 1.05
                }
                .min(Self::MAX);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn timeouts_and_late_acks_never_age_the_baseline_or_reward_recovery() {
        let start = Instant::now();
        let mut qos = RelayQos::new();
        qos.on_delay(Duration::from_millis(80), start);
        for i in 3..=60 {
            qos.on_stall(start + Duration::from_secs(i));
        }
        let cut = qos.bitrate();
        qos.on_ack(start, start + Duration::from_secs(61));
        assert_eq!(qos.bitrate(), cut);
        assert_eq!(qos.baseline, Some(80));
        assert_eq!(qos.samples.len(), 1);
        for i in 0..20 {
            let now = start + Duration::from_millis(62000 + i);
            qos.on_ack(now.checked_sub(Duration::from_millis(80)).unwrap(), now);
        }
        assert_eq!(qos.bitrate(), cut, "one ACK burst is not a stable recovery");
        for i in 0..20 {
            let now = start + Duration::from_millis(63000 + i * 100);
            qos.on_ack(now.checked_sub(Duration::from_millis(80)).unwrap(), now);
        }
        assert!(qos.bitrate() > cut);
    }
    #[test]
    fn first_expired_ack_cuts_without_inventing_a_path_delay() {
        let now = Instant::now();
        let mut qos = RelayQos::new();
        qos.on_ack(now.checked_sub(Duration::from_secs(3)).unwrap(), now);
        assert_eq!(qos.bitrate(), RelayQos::new().bitrate() / 2);
        assert_eq!(qos.baseline, None);
        assert!(qos.samples.is_empty());
        qos.on_ack(
            now.checked_sub(Duration::from_secs(2)).unwrap(),
            now + Duration::from_millis(600),
        );
        assert_eq!(
            qos.bitrate(),
            RelayQos::new().bitrate() / 2,
            "late ACK cut twice"
        );
    }
    #[test]
    fn a_stable_higher_path_floor_is_learned_gradually_not_per_ack() {
        let start = Instant::now();
        let mut qos = RelayQos::new();
        qos.on_delay(Duration::from_millis(80), start);
        for _ in 0..100 {
            qos.on_delay(Duration::from_millis(500), start + Duration::from_secs(1));
        }
        assert_eq!(qos.baseline, Some(105));
        for i in 2..=18 {
            qos.on_delay(Duration::from_millis(500), start + Duration::from_secs(i));
        }
        assert_eq!(qos.baseline, Some(500));
        qos.on_delay(Duration::from_millis(60), start + Duration::from_secs(19));
        assert_eq!(qos.baseline, Some(60), "lower delays apply immediately");
        assert_eq!(qos.samples.len(), RelayQos::SAMPLES);
    }

    #[test]
    fn slowly_growing_queues_do_not_age_the_path_floor() {
        let start = Instant::now();
        let mut qos = RelayQos::new();
        qos.on_delay(Duration::from_millis(80), start);
        for i in 1..=120 {
            qos.on_delay(
                Duration::from_millis(80 + i),
                start + Duration::from_millis(i * 50),
            );
        }
        assert_eq!(qos.baseline, Some(80));
    }

    #[test]
    fn an_existing_queue_is_not_spare_capacity() {
        let start = Instant::now();
        let mut qos = RelayQos::new();
        qos.on_delay(Duration::from_millis(80), start);
        for i in 1..=20 {
            qos.on_delay(
                Duration::from_millis(160),
                start + Duration::from_millis(i * 50),
            );
        }
        assert_eq!(qos.bitrate(), RelayQos::new().bitrate());
        assert!(qos.proven <= RelayQos::MIN);
    }

    #[test]
    fn replies_from_a_previous_target_cannot_validate_the_current_target() {
        let now = Instant::now();
        let mut qos = RelayQos::new();
        // A fast path may deliver a batch of old frames immediately after a
        // target change. None of those frames was sent at the new target.
        qos.last_rise = Some(now);
        for _ in 0..20 {
            qos.on_ack(
                now.checked_sub(Duration::from_millis(10)).unwrap(),
                now + Duration::from_millis(10),
            );
        }
        assert_eq!(qos.good, 0);
        assert!(qos.clean_since.is_none());
        assert!(qos.proven <= RelayQos::MIN);
        qos.last_rise = None;
        qos.last_cut = Some(now);
        qos.on_ack(
            now.checked_sub(Duration::from_millis(10)).unwrap(),
            now + Duration::from_millis(10),
        );
        assert_eq!(qos.good, 0, "the reduced rate also needs its own feedback");
    }

    #[test]
    fn one_low_delay_reply_does_not_prove_an_entire_observation_window() {
        let start = Instant::now();
        let mut qos = RelayQos::new();
        qos.on_delay(Duration::from_millis(80), start);
        for i in 1..10 {
            qos.on_delay(
                Duration::from_millis(120),
                start + Duration::from_millis(i * 100),
            );
        }
        qos.on_delay(Duration::from_millis(80), start + Duration::from_secs(1));
        assert!(
            qos.bitrate() > RelayQos::new().bitrate(),
            "small probe is allowed"
        );
        assert!(
            qos.proven <= RelayQos::MIN,
            "queue disappeared only in the final sample"
        );
    }

    #[test]
    fn recovery_is_bounded_by_the_last_failed_level_until_new_samples_validate_it() {
        let start = Instant::now();
        let mut qos = RelayQos::new();
        for i in 0..=10 {
            qos.on_delay(
                Duration::from_millis(80),
                start + Duration::from_millis(i * 100),
            );
        }
        let proven = qos.proven;
        qos.on_stall(start + Duration::from_secs(2));
        let failed = qos.congested.unwrap();
        let cut = qos.bitrate();
        for i in 0..=10 {
            let now = start + Duration::from_millis(3000 + i * 100);
            qos.on_ack(now.checked_sub(Duration::from_millis(80)).unwrap(), now);
        }
        assert!(
            qos.bitrate() > cut + cut / 10,
            "clean recovery should exceed a small probe"
        );
        assert!(qos.bitrate <= proven);
        // Continued clean observations recover towards, but do not jump over,
        // the remembered failure. Re-testing it uses small probes.
        for i in 11..=80 {
            let now = start + Duration::from_millis(3000 + i * 100);
            qos.on_ack(now.checked_sub(Duration::from_millis(80)).unwrap(), now);
            if qos.congested.is_some() {
                assert!(qos.bitrate <= failed * 1.05);
            }
        }
        assert!(
            qos.congested.is_none(),
            "fresh clean observations can remove the guard"
        );
    }
}
