//! The vendor's extra answer request belongs to its original prompt, even when
//! its receiver or acknowledgement is delayed across a warm session handoff.
use super::{DriverEvent, Shared, turn};
use std::sync::{Arc, Weak, atomic::Ordering};

pub(super) struct Bridge {
    owner: Weak<Shared>,
    turn: Arc<turn::Turn>,
}
impl Bridge {
    /// Caller already holds lifecycle while admitting the native question.
    pub fn capture(owner: &Arc<Shared>) -> Option<Self> {
        Some(Self {
            owner: Arc::downgrade(owner),
            turn: owner.latest_turn.lock().expect("ACP turn").clone()?,
        })
    }
    /// Enqueue the native request while holding the same gate as prompt
    /// settlement/admission. Waiting for its response never holds that gate.
    pub fn with_current<T>(&self, apply: impl FnOnce() -> T) -> Option<T> {
        let owner = self.owner.upgrade()?;
        let _gate = owner.lifecycle.lock().expect("ACP lifecycle");
        if owner.setup_in_progress.load(Ordering::Acquire)
            || self.turn.epoch.load(Ordering::Acquire)
            || self.turn.retiring.load(Ordering::Acquire)
            || self.turn.done.borrow().is_some()
            || owner
                .validate_interaction(Some(&self.turn.session))
                .is_err()
            || !owner
                .latest_turn
                .lock()
                .expect("ACP turn")
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, &self.turn))
        {
            return None;
        }
        Some(apply())
    }
    pub fn delivery_failed(&self, id: String) {
        self.with_current(|| {
            // Do not hold a sender while awaiting the vendor: the old timeline
            // must still close promptly when its own terminal result arrives.
            if let Some(owner) = self.owner.upgrade()
                && let Some(updates) = owner.updates.lock().expect("updates").as_ref()
            {
                let _ = updates.send(DriverEvent::ProtocolWarning {
                    message: "CodeBuddy did not confirm delivery of the question answer".into(),
                });
                let _ = updates.send(DriverEvent::QuestionClosed {
                    native_request_id: id,
                });
            }
        });
    }
}
