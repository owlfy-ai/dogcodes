//! Switch physical output and fence publishing until the worker applies a new
//! encoder profile. Auto-level frames must never leak into a CB3.1 track.
use super::{Arc, EncodeAdmission, EncodedFrame, Instant, Mutex, VideoOut};
use dogcodes_remote_desktop::video::VideoProfile;
struct State {
    output: Arc<dyn VideoOut>,
    profile: VideoProfile,
    codec: Option<VideoProfile>,
    generation: u64,
    hold: Option<Instant>,
    ready: bool,
    required: bool,
    resync: bool,
    ended: Option<Instant>,
}
pub(super) struct Output {
    state: Mutex<State>,
}
impl Output {
    pub fn new(output: Arc<dyn VideoOut>) -> Self {
        Self {
            state: Mutex::new(State {
                generation: output.video_epoch().unwrap_or(0),
                profile: output.profile(),
                codec: Some(output.profile()),
                output,
                hold: None,
                ready: true,
                required: false,
                resync: false,
                ended: None,
            }),
        }
    }
    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    pub fn original(&self) -> Arc<dyn VideoOut> {
        self.state().output.clone()
    }
    pub fn hold(&self, until: Option<Instant>) -> Result<(), String> {
        let mut state = self.state();
        if until.is_some() && state.ended.is_some() {
            return Err("旧视频管线已结束，请重新连接".into());
        }
        state.hold = until;
        Ok(())
    }
    pub fn ready(&self) -> bool {
        let state = self.state();
        state.ready
            && state.codec == Some(state.profile)
            && state.ended.is_none()
            && state.output.ready()
    }
    pub fn required(&self) -> bool {
        let s = self.state();
        s.required || s.output.ready_required()
    }
    pub fn replace(&self, output: Arc<dyn VideoOut>) -> Result<u64, String> {
        let mut state = self.state();
        if state.ended.is_some() {
            return Err("旧视频管线已结束，请重新连接".into());
        }
        let next = state
            .generation
            .max(state.output.video_epoch().unwrap_or(0))
            .checked_add(1)
            .ok_or("请重新开启视频租约")?;
        if output.video_epoch().is_some_and(|epoch| epoch != next) {
            return Err("新视频发布代次不匹配".into());
        }
        state.output.retire();
        if state.profile != output.profile() {
            state.codec = None;
        }
        state.profile = output.profile();
        state.output = output;
        state.generation = next;
        state.ready = false;
        state.required = true;
        state.resync = true;
        state.hold = None;
        Ok(next)
    }
    /// A replacement pipeline always waits for this decoder generation and
    /// for the worker to confirm its actual encoder profile.
    pub fn rebuild(&self) {
        let mut s = self.state();
        s.output.reset();
        s.generation = s
            .output
            .video_epoch()
            .unwrap_or_else(|| s.generation.wrapping_add(1));
        s.codec = None;
        s.ready = false;
        s.required = true;
        s.resync = true;
    }
    pub fn codec_ready(&self) -> bool {
        let state = self.state();
        state.codec == Some(state.profile)
    }
    pub fn migrated(&self, generation: u64) {
        let mut state = self.state();
        state.generation = generation;
        state.ready = false;
        state.required = true;
        state.resync = true;
    }
    pub fn activate(&self, generation: u64) -> Result<(), String> {
        let mut state = self.state();
        if state.ended.is_some() {
            return Err("视频管线已结束".into());
        }
        if state.generation != generation {
            return Err("视频代次已变化，请重新准备画面".into());
        }
        if !state.ready {
            state.resync = true;
            state.output.begin_recovery();
            state.ready = true;
        }
        Ok(())
    }
}
impl VideoOut for Output {
    fn begin_recovery(&self) {
        let mut state = self.state();
        state.resync = true;
        state.output.begin_recovery();
    }
    fn admission(&self) -> EncodeAdmission {
        let state = self.state();
        if state.ended.is_some() || !state.ready || state.codec != Some(state.profile) {
            return EncodeAdmission::Wait;
        }
        match state.output.admission() {
            EncodeAdmission::Frame if state.resync => EncodeAdmission::Keyframe,
            permit => permit,
        }
    }
    fn profile(&self) -> VideoProfile {
        self.state().profile
    }
    fn codec_applied(&self, profile: VideoProfile) {
        let mut state = self.state();
        if state.profile == profile {
            state.codec = Some(profile);
            state.resync = true;
            state.output.begin_recovery();
        }
    }
    fn video_epoch(&self) -> Option<u64> {
        let s = self.state();
        s.output.video_epoch().or(Some(s.generation))
    }
    fn can_resize(&self) -> bool {
        let s = self.state();
        s.ready && (s.codec == Some(s.profile)) && s.output.can_resize()
    }
    fn push(&self, frame: EncodedFrame, captured: Instant) -> bool {
        let mut s = self.state();
        if s.ended.is_some()
            || !s.ready
            || s.codec != Some(s.profile)
            || s.resync && !frame.keyframe
        {
            return false;
        }
        let key = frame.keyframe;
        let accepted = s.output.push(frame, captured);
        if !accepted {
            s.resync = true;
        } else if key {
            s.resync = false;
        }
        accepted
    }
    fn target_bitrate(&self) -> u64 {
        self.state().output.target_bitrate()
    }
    fn take_keyframe_request(&self) -> bool {
        self.state().output.take_keyframe_request()
    }
    fn reset(&self) {
        let mut s = self.state();
        s.output.reset();
        s.generation = s.generation.wrapping_add(1);
        s.resync = true;
    }
    fn closed(&self) -> bool {
        let mut s = self.state();
        if s.output.closed() && s.hold.is_none_or(|until| Instant::now() >= until) {
            s.ended.get_or_insert_with(Instant::now);
        }
        s.ended.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    struct Probe {
        profile: VideoProfile,
        closed: AtomicBool,
        frames: Mutex<Vec<bool>>,
        credit: AtomicBool,
    }
    impl Probe {
        fn new(profile: VideoProfile) -> Arc<Self> {
            Arc::new(Self {
                profile,
                closed: AtomicBool::new(false),
                frames: Mutex::new(Vec::new()),
                credit: AtomicBool::new(true),
            })
        }
    }
    impl VideoOut for Probe {
        fn admission(&self) -> EncodeAdmission {
            if self.credit.load(Ordering::Acquire) && !self.closed() {
                EncodeAdmission::Frame
            } else {
                EncodeAdmission::Wait
            }
        }
        fn profile(&self) -> VideoProfile {
            self.profile
        }
        fn push(&self, frame: EncodedFrame, _at: Instant) -> bool {
            self.frames.lock().unwrap().push(frame.keyframe);
            true
        }
        fn target_bitrate(&self) -> u64 {
            800_000
        }
        fn take_keyframe_request(&self) -> bool {
            false
        }
        fn closed(&self) -> bool {
            self.closed.load(Ordering::Acquire)
        }
    }
    fn frame(keyframe: bool) -> EncodedFrame {
        EncodedFrame {
            cursor_embedded: None,
            captured: std::time::Instant::now(),
            keyframe,
            data: vec![0; 10],
            latency: std::time::Duration::ZERO,
        }
    }
    #[test]
    fn rtp_swap_keeps_profile_requires_the_new_epoch_ready_and_starts_from_idr() {
        let old = Probe::new(VideoProfile::ConstrainedBaseline31);
        let new = Probe::new(VideoProfile::ConstrainedBaseline31);
        let output = Output::new(old.clone());
        assert!(output.push(frame(true), Instant::now()));
        output
            .hold(Instant::now().checked_add(std::time::Duration::from_secs(20)))
            .unwrap();
        old.closed.store(true, Ordering::Release);
        assert!(!output.closed());
        let epoch = output.replace(new.clone()).unwrap();
        assert_eq!(epoch, 1);
        assert!(!output.push(frame(true), Instant::now()));
        assert!(output.activate(0).is_err());
        output.activate(epoch).unwrap();
        assert!(!output.push(frame(false), Instant::now()));
        assert!(output.push(frame(true), Instant::now()));
        assert!(output.push(frame(false), Instant::now()));
        output.activate(epoch).unwrap();
        assert!(
            output.admission() == EncodeAdmission::Frame,
            "ready retry rearmed a completed recovery"
        );
        assert_eq!(*old.frames.lock().unwrap(), [true]);
        assert_eq!(*new.frames.lock().unwrap(), [true, false]);
        assert!(!output.closed());
        new.closed.store(true, Ordering::Release);
        assert!(output.closed());
    }
    #[test]
    fn a_worker_that_already_observed_closed_cannot_be_revived_by_late_stage() {
        let old = Probe::new(VideoProfile::ConstrainedBaseline31);
        let output = Output::new(old.clone());
        old.closed.store(true, Ordering::Release);
        assert!(output.closed());
        assert!(
            output
                .hold(Instant::now().checked_add(std::time::Duration::from_secs(20)))
                .is_err()
        );
        assert!(
            output
                .replace(Probe::new(VideoProfile::ConstrainedBaseline31))
                .is_err()
        );
    }
    #[test]
    fn profile_switch_cannot_publish_until_the_new_codec_is_confirmed() {
        let original = Probe::new(VideoProfile::ConstrainedBaselineAuto);
        let output = Output::new(original.clone());
        let receiver = Probe::new(VideoProfile::ConstrainedBaseline31);
        let epoch = output.replace(receiver.clone()).unwrap();
        output.activate(epoch).unwrap();
        assert!(!output.push(frame(true), Instant::now()));
        assert!(!output.codec_ready());
        output.codec_applied(VideoProfile::ConstrainedBaselineAuto);
        assert!(!output.push(frame(true), Instant::now()));
        output.codec_applied(VideoProfile::ConstrainedBaseline31);
        assert!(!output.push(frame(false), Instant::now()));
        assert!(output.push(frame(true), Instant::now()));
        assert!(original.frames.lock().unwrap().is_empty());
        assert_eq!(*receiver.frames.lock().unwrap(), [true]);
    }

    #[test]
    fn recovery_credit_tracks_the_replacement_output_and_its_ready_codec() {
        let old = Probe::new(VideoProfile::ConstrainedBaselineAuto);
        let output = Output::new(old.clone());
        output.begin_recovery();
        old.credit.store(false, Ordering::Release);
        assert!(output.admission() == EncodeAdmission::Wait);
        let new = Probe::new(VideoProfile::ConstrainedBaseline31);
        new.credit.store(false, Ordering::Release);
        let epoch = output.replace(new.clone()).unwrap();
        old.credit.store(true, Ordering::Release);
        assert!(output.admission() == EncodeAdmission::Wait);
        output.activate(epoch).unwrap();
        assert!(output.admission() == EncodeAdmission::Wait);
        output.codec_applied(VideoProfile::ConstrainedBaseline31);
        assert!(output.admission() == EncodeAdmission::Wait);
        new.credit.store(true, Ordering::Release);
        assert!(output.admission() == EncodeAdmission::Keyframe);
        assert!(!output.push(frame(false), Instant::now()));
        assert!(output.push(frame(true), Instant::now()));
        assert!(output.admission() == EncodeAdmission::Frame);
        assert!(old.frames.lock().unwrap().is_empty());
        assert_eq!(*new.frames.lock().unwrap(), [true]);
    }
}
