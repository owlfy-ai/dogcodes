//! Bounded static refinement and a watchdog driven by real codec output.
use std::time::{Duration, Instant};

const QUIET: Duration = Duration::from_millis(400);
const SPACING: Duration = Duration::from_millis(100);
const OUTPUT_TIMEOUT: Duration = Duration::from_secs(2);
const REFRESH_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_ATTEMPTS: u8 = 24;
const MAX_OUTPUTS: u64 = 12;

#[derive(Clone, Copy, Default, Debug)]
pub(super) struct Output {
    pub sequence: u64,
    pub tiny_run: u8,
    pub keyframes: u64,
}
impl Output {
    pub fn emitted(&mut self, bytes: usize, keyframe: bool) {
        self.sequence = self.sequence.saturating_add(1);
        self.keyframes = self.keyframes.saturating_add(u64::from(keyframe));
        // This is only an early-stop heuristic, never a quality guarantee.
        // Repeated tiny P access units cannot justify unlimited conversion.
        self.tiny_run = if bytes <= 64 {
            self.tiny_run.saturating_add(1)
        } else {
            0
        };
    }
}

#[derive(Debug)]
pub(super) struct Refresh {
    changed: Option<Instant>,
    last_attempt: Option<Instant>,
    automatic: Option<(Instant, u64)>,
    attempts: u8,
    observed: u64,
    pending: Option<Instant>,
    key_pending: Option<(Instant, u64)>,
}
impl Refresh {
    pub fn new() -> Self {
        Self {
            changed: None,
            last_attempt: None,
            automatic: None,
            attempts: 0,
            observed: 0,
            pending: None,
            key_pending: None,
        }
    }
    pub fn changed(&mut self, now: Instant) {
        self.changed = Some(now);
        self.automatic = None;
        self.attempts = 0;
        // New motion must not renew a codec's no-output deadline.
    }
    fn observe(&mut self, output: Output) {
        if self
            .key_pending
            .is_some_and(|(_, keys)| output.keyframes > keys)
        {
            self.key_pending = None;
        }
        if output.sequence > self.observed {
            self.observed = output.sequence;
            self.pending = None;
        }
    }
    pub fn submitting(&mut self, now: Instant, output: Output, key: bool) {
        self.observe(output);
        self.pending.get_or_insert(now);
        if key {
            self.key_pending.get_or_insert((now, output.keyframes));
        }
        self.last_attempt = Some(now);
    }
    pub fn backpressured(&mut self) {
        // Retain the caller's IDR intent, but its next delivery deadline can
        // start only when a new input is admitted. The native-output deadline
        // remains intact so a blocked transport cannot hide a hung codec job.
        self.key_pending = None;
    }
    #[cfg(test)]
    pub fn tick(
        &mut self,
        now: Instant,
        output: Output,
        key: bool,
        fps: u32,
    ) -> Result<Option<bool>, String> {
        self.tick_admitted(now, output, key, fps, true, false)
    }
    /// None means idle; Some(force_idr) authorizes one paced cached-frame attempt.
    pub fn tick_admitted(
        &mut self,
        now: Instant,
        output: Output,
        key: bool,
        fps: u32,
        admitted: bool,
        latest_pending: bool,
    ) -> Result<Option<bool>, String> {
        self.observe(output);
        if self
            .pending
            .is_some_and(|since| now.duration_since(since) >= OUTPUT_TIMEOUT)
        {
            return Err("视频编码器持续未输出画面，请重新连接".into());
        }
        if self
            .key_pending
            .is_some_and(|(since, _)| now.duration_since(since) >= OUTPUT_TIMEOUT)
        {
            return Err("视频编码器未能输出关键帧，请重新连接".into());
        }
        let key = key || self.key_pending.is_some();
        if !admitted {
            return Ok(None);
        }
        let Some(changed) = self.changed else {
            return Ok(None);
        };
        let interval = (Duration::from_secs(1) / fps.clamp(1, 60)).max(if key {
            Duration::from_millis(60)
        } else if latest_pending {
            Duration::ZERO
        } else {
            SPACING
        });
        if self
            .last_attempt
            .is_some_and(|last| now.duration_since(last) < interval)
        {
            return Ok(None);
        }
        // Loss recovery remains available after the automatic budget is spent.
        if key {
            return Ok(Some(true));
        }
        if latest_pending {
            return Ok(Some(false));
        }
        if now.duration_since(changed) < QUIET {
            return Ok(None);
        }
        let (started, baseline) = *self.automatic.get_or_insert((now, output.sequence));
        let outputs = output.sequence.saturating_sub(baseline);
        if self.attempts >= MAX_ATTEMPTS
            || outputs >= MAX_OUTPUTS
            || now.duration_since(started) >= REFRESH_TIMEOUT
            || output.tiny_run.min(outputs.min(u64::from(u8::MAX)) as u8) >= 3
        {
            return Ok(None);
        }
        self.attempts += 1;
        Ok(Some(false))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn denied_inputs_do_not_start_deadlines_or_spend_refinement_budget() {
        let start = Instant::now();
        let mut state = Refresh::new();
        state.changed(start);
        for i in 1..=100 {
            assert_eq!(
                state
                    .tick_admitted(
                        start + Duration::from_secs(i),
                        Output::default(),
                        true,
                        30,
                        false,
                        true
                    )
                    .unwrap(),
                None
            );
        }
        assert_eq!(
            state
                .tick_admitted(
                    start + Duration::from_secs(101),
                    Output::default(),
                    true,
                    30,
                    true,
                    true
                )
                .unwrap(),
            Some(true)
        );
        state.submitting(start + Duration::from_secs(101), Output::default(), true);
        assert!(
            state
                .tick_admitted(
                    start + Duration::from_secs(104),
                    Output::default(),
                    true,
                    30,
                    false,
                    true
                )
                .is_err(),
            "backpressure must not hide a genuinely stalled native call"
        );
    }

    #[test]
    fn a_completed_old_input_does_not_expire_a_not_yet_admitted_recovery_idr() {
        let start = Instant::now();
        let mut state = Refresh::new();
        let mut output = Output::default();
        state.changed(start);
        state.submitting(start, output, true);
        state.backpressured();
        output.emitted(1000, false); // Old output, discarded by the flow fence.
        assert_eq!(
            state
                .tick_admitted(
                    start + Duration::from_secs(100),
                    output,
                    true,
                    30,
                    false,
                    true
                )
                .unwrap(),
            None
        );
        assert_eq!(
            state
                .tick_admitted(
                    start + Duration::from_secs(101),
                    output,
                    true,
                    30,
                    true,
                    true
                )
                .unwrap(),
            Some(true)
        );
    }
    #[test]
    fn new_motion_and_successful_submissions_cannot_hide_a_silent_encoder() {
        let start = Instant::now();
        let mut state = Refresh::new();
        for i in 0..20 {
            let now = start + Duration::from_millis(i * 100);
            state.changed(now);
            state.submitting(now, Output::default(), false);
            assert!(
                !state
                    .tick(now, Output::default(), false, 30)
                    .unwrap()
                    .is_some()
            );
        }
        assert!(
            state
                .tick(start + OUTPUT_TIMEOUT, Output::default(), false, 30)
                .is_err()
        );
    }
    #[test]
    fn rate_control_skips_retry_at_bounded_speed_then_report_missing_output() {
        let start = Instant::now();
        let mut state = Refresh::new();
        state.changed(start);
        state.submitting(start, Output::default(), true);
        let mut calls = 0;
        for i in 1..200 {
            let now = start + Duration::from_millis(i * 10);
            if state
                .tick(now, Output::default(), true, 5)
                .unwrap()
                .is_some()
            {
                calls += 1;
                state.submitting(now, Output::default(), false);
            }
        }
        assert_eq!(calls, 9, "keyframe retries must respect the negotiated FPS");
        assert!(
            state
                .tick(start + OUTPUT_TIMEOUT, Output::default(), true, 5)
                .is_err()
        );
    }
    #[test]
    fn actual_output_allows_bounded_refinement_and_later_keyframe_recovery() {
        let start = Instant::now();
        let mut state = Refresh::new();
        let mut output = Output::default();
        state.changed(start);
        state.submitting(start, output, false);
        output.emitted(2000, false);
        let mut calls = 0;
        for i in 1..100 {
            let now = start + Duration::from_millis(i * 100);
            if state.tick(now, output, false, 30).unwrap().is_some() {
                calls += 1;
                state.submitting(now, output, false);
                output.emitted(1000, false);
            }
        }
        assert_eq!(calls, 12);
        assert!(
            state
                .tick(start + Duration::from_secs(10), output, true, 30)
                .unwrap()
                .is_some()
        );
        state.changed(start + Duration::from_secs(10));
        assert!(
            state
                .tick(start + Duration::from_millis(10400), output, false, 30)
                .unwrap()
                .is_some()
        );
    }
    #[test]
    fn tiny_outputs_before_static_refinement_do_not_spend_its_budget() {
        let start = Instant::now();
        let mut state = Refresh::new();
        let mut output = Output::default();
        for _ in 0..5 {
            output.emitted(13, false);
        }
        state.changed(start);
        for i in 4..7 {
            let now = start + Duration::from_millis(i * 100);
            assert!(state.tick(now, output, false, 30).unwrap().is_some());
            state.submitting(now, output, false);
            output.emitted(13, false);
        }
        assert!(
            !state
                .tick(start + Duration::from_millis(700), output, false, 30)
                .unwrap()
                .is_some()
        );
        assert!(
            !state
                .tick(start + Duration::from_secs(4), output, false, 30)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn p_frames_cannot_acknowledge_a_keyframe_or_renew_its_deadline() {
        let start = Instant::now();
        let mut state = Refresh::new();
        let mut output = Output::default();
        state.changed(start);
        state.submitting(start, output, true);
        for i in 1..20 {
            let now = start + Duration::from_millis(i * 100);
            output.emitted(1000, false);
            state.changed(now);
            assert_eq!(state.tick(now, output, false, 30).unwrap(), Some(true));
            state.submitting(now, output, true);
        }
        output.emitted(1000, false);
        let error = state
            .tick(start + OUTPUT_TIMEOUT, output, false, 30)
            .unwrap_err();
        assert!(error.contains("关键帧"));
    }

    #[test]
    fn an_actual_idr_satisfies_recovery_without_permanent_keyframe_retries() {
        let start = Instant::now();
        let mut state = Refresh::new();
        let mut output = Output::default();
        state.changed(start);
        state.submitting(start, output, true);
        output.emitted(1000, true);
        assert_eq!(
            state
                .tick(start + Duration::from_millis(100), output, false, 30)
                .unwrap(),
            None
        );
        assert_eq!(
            state
                .tick(start + Duration::from_millis(400), output, false, 30)
                .unwrap(),
            Some(false)
        );
    }
}
