//! The final native callback boundary, shared by sync and async encoders.
use super::{Result, Sink};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

#[cfg(test)]
pub(super) fn guard(
    sink: Sink,
    stopped: Arc<AtomicBool>,
    error: Arc<Mutex<Option<String>>>,
    check: impl Fn() -> Result<()> + Send + Sync + 'static,
) -> Sink {
    guard_when(sink, stopped, error, || true, check)
}

pub(super) fn guard_when(
    sink: Sink,
    stopped: Arc<AtomicBool>,
    error: Arc<Mutex<Option<String>>>,
    publishable: impl Fn() -> bool + Send + Sync + 'static,
    check: impl Fn() -> Result<()> + Send + Sync + 'static,
) -> Sink {
    Arc::new(move |frame| {
        if stopped.load(Ordering::Acquire) || !publishable() {
            return;
        }
        if let Err(reason) = check() {
            stopped.store(true, Ordering::Release);
            error
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get_or_insert(reason);
            return;
        }
        // Stop may have happened while the OS query was pending. No cached
        // pre-encode grant authorizes a late codec callback to publish.
        if !stopped.load(Ordering::Acquire) && publishable() {
            sink(frame);
        }
    })
}

#[cfg(test)]
mod tests {
    use super::super::EncodedFrame;
    use super::*;
    use std::{sync::atomic::AtomicUsize, time::Duration};
    fn frame() -> EncodedFrame {
        EncodedFrame {
            cursor_embedded: None,
            captured: std::time::Instant::now(),
            data: vec![1],
            keyframe: true,
            latency: Duration::ZERO,
        }
    }
    #[test]
    fn a_geometry_change_during_authorization_fences_late_output_without_revoking_the_grant() {
        let changed = Arc::new(AtomicBool::new(false));
        let gate = changed.clone();
        let update = changed.clone();
        let stopped = Arc::new(AtomicBool::new(false));
        let error = Arc::new(Mutex::new(None));
        let sink = guard_when(
            Arc::new(|_| panic!("old geometry escaped")),
            stopped.clone(),
            error.clone(),
            move || !gate.load(Ordering::Acquire),
            move || {
                update.store(true, Ordering::Release);
                Ok(())
            },
        );
        sink(frame());
        sink(frame());
        assert!(!stopped.load(Ordering::Acquire));
        assert!(error.lock().unwrap().is_none());
    }
    #[test]
    fn a_failed_grant_retires_output_before_waiting_for_diagnostics() {
        let stopped = Arc::new(AtomicBool::new(false));
        let error = Arc::new(Mutex::new(None));
        let held = error.lock().unwrap();
        let (entered, checking) = std::sync::mpsc::sync_channel(1);
        let sink = guard(
            Arc::new(|_| panic!("denied output")),
            stopped.clone(),
            error.clone(),
            move || {
                entered.send(()).unwrap();
                Err("locked".into())
            },
        );
        let worker = std::thread::spawn(move || sink(frame()));
        checking.recv_timeout(Duration::from_secs(1)).unwrap();
        let until = std::time::Instant::now() + Duration::from_millis(100);
        while !stopped.load(Ordering::Acquire) && std::time::Instant::now() < until {
            std::thread::yield_now();
        }
        let retired = stopped.load(Ordering::Acquire);
        drop(held);
        worker.join().unwrap();
        assert!(retired, "diagnostic contention delayed stream retirement");
    }
    #[test]
    fn a_late_codec_callback_after_stop_does_not_query_or_publish() {
        let calls = Arc::new(AtomicUsize::new(0));
        let check_calls = calls.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let errors = Arc::new(Mutex::new(None));
        let sink = guard(
            Arc::new(|_| panic!("retired output")),
            stop.clone(),
            errors,
            move || {
                check_calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
        );
        stop.store(true, Ordering::Release);
        sink(frame());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }
    #[test]
    fn stop_during_the_post_encode_query_retires_the_just_confirmed_output() {
        let stop = Arc::new(AtomicBool::new(false));
        let closing = stop.clone();
        let sink = guard(
            Arc::new(|_| panic!("late output")),
            stop,
            Arc::new(Mutex::new(None)),
            move || {
                closing.store(true, Ordering::Release);
                Ok(())
            },
        );
        sink(frame());
    }
    #[test]
    fn lock_or_unknown_state_ends_the_stream_and_does_not_resume_on_a_later_grant() {
        for reason in ["电脑已锁定", "状态未知"] {
            let calls = Arc::new(AtomicUsize::new(0));
            let checking = calls.clone();
            let output = Arc::new(AtomicUsize::new(0));
            let sent = output.clone();
            let error = Arc::new(Mutex::new(None));
            let stop = Arc::new(AtomicBool::new(false));
            let sink = guard(
                Arc::new(move |_| {
                    sent.fetch_add(1, Ordering::SeqCst);
                }),
                stop.clone(),
                error.clone(),
                move || {
                    if checking.fetch_add(1, Ordering::SeqCst) == 1 {
                        Err(reason.into())
                    } else {
                        Ok(())
                    }
                },
            );
            sink(frame());
            sink(frame());
            sink(frame());
            assert_eq!(output.load(Ordering::SeqCst), 1);
            assert_eq!(calls.load(Ordering::SeqCst), 2);
            assert!(stop.load(Ordering::Acquire));
            assert_eq!(error.lock().unwrap().as_deref(), Some(reason));
        }
    }
}
