//! Latest-input flow credit and a fence for callbacks from before a pause.
use super::{EncodeAdmission, EncodeGate, Sink, refresh::Output};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};
pub(super) struct Flow {
    gate: EncodeGate,
    blocked: AtomicBool,
    since: Arc<Mutex<Option<Instant>>>,
}
impl Flow {
    pub fn new(gate: EncodeGate) -> Self {
        Self {
            gate,
            blocked: AtomicBool::new(false),
            since: Arc::new(Mutex::new(None)),
        }
    }
    /// Called under the encoder lock, so source/cache transitions are ordered.
    pub fn admission(&self, now: Instant) -> EncodeAdmission {
        let permit = (self.gate)();
        let blocked = permit == EncodeAdmission::Wait;
        let previous = self.blocked.swap(blocked, Ordering::AcqRel);
        if previous != blocked {
            *self.since.lock().unwrap_or_else(|e| e.into_inner()) = Some(now);
        }
        if previous && !blocked {
            EncodeAdmission::Keyframe
        } else {
            permit
        }
    }
    pub fn track(&self, sink: Sink, output: Arc<Mutex<Output>>) -> Sink {
        let since = self.since.clone();
        Arc::new(move |frame| {
            // Keep the freshness check and publication linear with pause/resume.
            let boundary = since.lock().unwrap_or_else(|e| e.into_inner());
            let fresh = boundary.is_none_or(|at| frame.captured >= at);
            output
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .emitted(frame.data.len(), frame.keyframe && fresh);
            // Stale output is still real codec progress, but cannot satisfy a
            // fresh IDR request or leak an old picture after flow resumes.
            if fresh {
                sink(frame);
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::video::EncodedFrame;
    use std::time::Duration;
    #[test]
    fn pre_pause_callbacks_cannot_publish_or_acknowledge_a_new_idr() {
        let gate = Arc::new(AtomicBool::new(true));
        let permission = gate.clone();
        let flow = Flow::new(Arc::new(move || {
            if permission.load(Ordering::Acquire) {
                EncodeAdmission::Frame
            } else {
                EncodeAdmission::Wait
            }
        }));
        let output = Arc::new(Mutex::new(Output::default()));
        let accepted = Arc::new(Mutex::new(Vec::new()));
        let frames = accepted.clone();
        let sink = flow.track(
            Arc::new(move |frame| frames.lock().unwrap().push(frame.captured)),
            output.clone(),
        );
        let old = Instant::now()
            .checked_sub(Duration::from_millis(1))
            .unwrap();
        gate.store(false, Ordering::Release);
        assert!(flow.admission(Instant::now()) == EncodeAdmission::Wait);
        let frame = |captured| EncodedFrame {
            cursor_embedded: None,
            captured,
            data: vec![0; 100],
            keyframe: true,
            latency: Duration::ZERO,
        };
        sink(frame(old));
        assert_eq!(output.lock().unwrap().sequence, 1);
        assert_eq!(output.lock().unwrap().keyframes, 0);
        gate.store(true, Ordering::Release);
        assert!(flow.admission(Instant::now()) == EncodeAdmission::Keyframe);
        sink(frame(old));
        sink(frame(Instant::now()));
        assert_eq!(accepted.lock().unwrap().len(), 1);
        assert_eq!(output.lock().unwrap().sequence, 3);
        assert_eq!(output.lock().unwrap().keyframes, 1);
    }
}
