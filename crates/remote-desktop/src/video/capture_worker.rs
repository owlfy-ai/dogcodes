//! Continuous native capture ownership. Native objects never leave their
//! worker; retiring a facade independently disconnects the real frame consumer.
use super::{Result, worker::Worker};
use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::Duration;

type Consumer<T> = Arc<dyn Fn(T) + Send + Sync>;
#[derive(Default)]
struct Phase {
    started: Option<Result<()>>,
    stopped: bool,
    error: Option<String>,
}
pub(super) struct Delivery<T> {
    active: AtomicBool,
    consumer: Mutex<Option<Consumer<T>>>,
    phase: Mutex<Phase>,
    changed: Condvar,
    on_retire: Arc<dyn Fn(Option<String>) + Send + Sync>,
    alive: Arc<dyn Fn() -> bool + Send + Sync>,
}
impl<T> Delivery<T> {
    #[cfg(test)]
    fn new(consumer: Consumer<T>) -> Self {
        Self::with_hook(consumer, Arc::new(|_| {}), Arc::new(|| true))
    }
    fn with_hook(
        consumer: Consumer<T>,
        on_retire: Arc<dyn Fn(Option<String>) + Send + Sync>,
        alive: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Self {
        Self {
            active: AtomicBool::new(true),
            consumer: Mutex::new(Some(consumer)),
            phase: Mutex::new(Phase::default()),
            changed: Condvar::new(),
            on_retire,
            alive,
        }
    }
    pub fn check_active(&self) -> Result<()> {
        if !(self.alive)() {
            self.retire();
        }
        if self.active.load(Ordering::Acquire) {
            Ok(())
        } else {
            Err(self.error().unwrap_or_else(|| "采集请求已结束".into()))
        }
    }
    pub fn after_prepare<P, C>(
        &self,
        prepare: impl FnOnce() -> Result<P>,
        start: impl FnOnce(P) -> Result<C>,
    ) -> Result<C> {
        self.check_active()?;
        let prepared = prepare()?;
        self.check_active()?;
        start(prepared)
    }
    pub fn emit(&self, frame: T) {
        if self.check_active().is_err() {
            return;
        }
        let consumer = self
            .consumer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if self.check_active().is_ok()
            && let Some(consumer) = consumer
        {
            consumer(frame);
        }
    }
    pub fn retire(&self) {
        self.active.store(false, Ordering::Release);
        // The production hook only sets the original stream's stop flag and
        // first error. It closes publication even for an already encoded frame.
        (self.on_retire)(self.error());
        // Do not hold this lock while calling the consumer. A frame already in
        // flight finishes under the VideoStream's final publication guard.
        self.consumer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
    }
    pub fn fail(&self, error: String) {
        self.phase
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .error
            .get_or_insert(error);
        self.retire();
    }
    pub fn error(&self) -> Option<String> {
        self.phase
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .error
            .clone()
    }
    pub fn started(&self, result: Result<()>) {
        let failed = result.is_err();
        {
            let mut phase = self.phase.lock().unwrap_or_else(|e| e.into_inner());
            if let Err(error) = &result {
                phase.error.get_or_insert_with(|| error.clone());
                phase.stopped = true;
            }
            phase.started.get_or_insert(result);
            self.changed.notify_all();
        }
        if failed {
            self.retire();
        }
    }
    /// Only a native start/stop completion or delegate may confirm this state.
    /// Facade cancellation alone must not release the native worker's slot.
    pub fn stopped(&self, error: Option<String>) {
        {
            let mut phase = self.phase.lock().unwrap_or_else(|e| e.into_inner());
            phase.stopped = true;
            if let Some(error) = error {
                phase.error.get_or_insert(error);
            }
            self.changed.notify_all();
        }
        self.retire();
    }
    pub fn is_stopped(&self) -> bool {
        self.phase.lock().unwrap_or_else(|e| e.into_inner()).stopped
    }
    /// These waits run only on the bounded native worker. The facade has its
    /// own deadline; keeping the context on timeout protects late callbacks.
    pub fn wait_started(&self) -> Result<()> {
        let mut phase = self.phase.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some(result) = &phase.started {
                return result.clone();
            }
            if phase.stopped {
                return Err(phase.error.clone().unwrap_or_else(|| "采集已结束".into()));
            }
            phase = self.changed.wait(phase).unwrap_or_else(|e| e.into_inner());
        }
    }
    pub fn wait_stopped(&self) {
        let mut phase = self.phase.lock().unwrap_or_else(|e| e.into_inner());
        while !phase.stopped {
            phase = self.changed.wait(phase).unwrap_or_else(|e| e.into_inner());
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct Info {
    pub size: (u32, u32),
    pub native: (u32, u32),
}
pub(super) struct Capture<T> {
    delivery: Arc<Delivery<T>>,
    worker: Mutex<Option<Worker<u32, ()>>>,
    info: Info,
}
impl<T: Send + 'static> Capture<T> {
    #[cfg(test)]
    pub fn start<C: 'static>(
        initialize: impl FnOnce(Arc<Delivery<T>>) -> Result<(C, Info)> + Send + 'static,
        process: impl FnMut(&mut C, u32) -> Result<()> + Send + 'static,
        consumer: impl Fn(T) + Send + Sync + 'static,
        initialize_timeout: Duration,
        timeout: Duration,
    ) -> Result<Self> {
        Self::start_with_hook(
            initialize,
            process,
            consumer,
            |_| {},
            Arc::new(|| true),
            initialize_timeout,
            timeout,
        )
    }
    pub fn start_with_hook<C: 'static>(
        initialize: impl FnOnce(Arc<Delivery<T>>) -> Result<(C, Info)> + Send + 'static,
        process: impl FnMut(&mut C, u32) -> Result<()> + Send + 'static,
        consumer: impl Fn(T) + Send + Sync + 'static,
        on_retire: impl Fn(Option<String>) + Send + Sync + 'static,
        alive: Arc<dyn Fn() -> bool + Send + Sync>,
        initialize_timeout: Duration,
        timeout: Duration,
    ) -> Result<Self> {
        let delivery = Arc::new(Delivery::with_hook(
            Arc::new(consumer),
            Arc::new(on_retire),
            alive,
        ));
        let starting = delivery.clone();
        let (metadata, received) = mpsc::sync_channel(1);
        let result = Worker::start_capture(
            move || {
                starting.check_active()?;
                let (native, info) = initialize(starting.clone())?;
                starting.check_active()?;
                metadata
                    .try_send(info)
                    .map_err(|_| "采集就绪接收者已结束")?;
                Ok(native)
            },
            process,
            initialize_timeout,
            timeout,
        );
        let worker = match result {
            Ok(worker) => worker,
            Err(error) => {
                delivery.fail(error.clone());
                return Err(delivery.error().unwrap_or(error));
            }
        };
        // Metadata is sent before the worker's readiness acknowledgement.
        let info = match received.try_recv() {
            Ok(info) => info,
            Err(_) => {
                delivery.fail("采集未返回尺寸".into());
                return Err("采集未返回尺寸".into());
            }
        };
        delivery.check_active()?;
        Ok(Self {
            delivery,
            worker: Mutex::new(Some(worker)),
            info,
        })
    }
    pub fn info(&self) -> Info {
        self.info
    }
    pub fn error(&self) -> Option<String> {
        self.delivery.error()
    }
    pub fn set_fps(&self, fps: u32) {
        if self.delivery.check_active().is_err() {
            return;
        }
        let mut worker = self.worker.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(active) = worker.as_mut()
            && let Err(error) = active.run(fps)
        {
            self.delivery.fail(error);
            worker.take();
        }
    }
    pub fn stop(&self) {
        self.delivery.retire();
        self.worker.lock().unwrap_or_else(|e| e.into_inner()).take();
    }
}
impl<T> Drop for Capture<T> {
    fn drop(&mut self) {
        self.delivery.retire();
        self.worker
            .get_mut()
            .unwrap_or_else(|e| e.into_inner())
            .take();
    }
}

#[cfg(test)]
mod tests {
    use super::super::worker;
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::time::Instant;

    fn info() -> Info {
        Info {
            size: (480, 320),
            native: (1440, 960),
        }
    }
    fn idle_capture() -> Capture<u32> {
        Capture::start(
            |delivery| {
                delivery.started(Ok(()));
                Ok(((), info()))
            },
            |_, _| Ok(()),
            |_| {},
            Duration::from_secs(1),
            Duration::from_secs(1),
        )
        .unwrap()
    }

    #[test]
    fn non_send_capture_state_stays_on_its_worker_and_metadata_is_cached() {
        let _serial = worker::isolated_capture();
        let caller = std::thread::current().id();
        let (exited, finished) = mpsc::channel();
        struct Native(
            std::rc::Rc<u32>,
            std::thread::ThreadId,
            Arc<Delivery<u32>>,
            mpsc::Sender<()>,
        );
        impl Drop for Native {
            fn drop(&mut self) {
                assert_eq!(self.1, std::thread::current().id());
                let _ = self.3.send(());
            }
        }
        let frames = Arc::new(Mutex::new(Vec::new()));
        let received = frames.clone();
        let capture = Capture::start(
            move |delivery| {
                let thread = std::thread::current().id();
                assert_ne!(thread, caller);
                delivery.started(Ok(()));
                Ok((
                    Native(std::rc::Rc::new(7), thread, delivery, exited),
                    info(),
                ))
            },
            |native, fps| {
                assert_eq!(native.1, std::thread::current().id());
                native.2.emit(*native.0 + fps);
                Ok(())
            },
            move |frame| received.lock().unwrap().push(frame),
            Duration::from_secs(1),
            Duration::from_secs(1),
        )
        .unwrap();
        assert_eq!(capture.info().size, (480, 320));
        assert_eq!(capture.info().native, (1440, 960));
        assert!(frames.lock().unwrap().is_empty());
        capture.set_fps(20);
        assert_eq!(*frames.lock().unwrap(), [27]);
        capture.stop();
        capture.stop();
        drop(capture);
        finished.recv_timeout(Duration::from_secs(1)).unwrap();
    }

    #[test]
    fn abandoned_content_reply_cannot_start_recording_and_keeps_its_slot_until_it_returns() {
        let _serial = worker::isolated_capture();
        let _codecs = worker::isolated();
        let (release, blocked) = mpsc::channel();
        let (observed, receiver) = mpsc::channel();
        let (exited, finished) = mpsc::channel();
        let attempts = Arc::new(AtomicUsize::new(0));
        let starts = attempts.clone();
        let owner = Arc::new(());
        let weak = Arc::downgrade(&owner);
        let publications = Arc::new(AtomicUsize::new(0));
        let published = publications.clone();
        let capture = Capture::start(
            move |delivery: Arc<Delivery<u32>>| {
                observed.send(delivery.clone()).unwrap();
                let result = delivery.after_prepare(
                    || {
                        blocked.recv().unwrap();
                        Ok(())
                    },
                    |_| {
                        starts.fetch_add(1, Ordering::AcqRel);
                        Ok(((), info()))
                    },
                );
                exited.send(()).unwrap();
                result
            },
            |_, _| Ok(()),
            move |_| {
                std::hint::black_box(&owner);
                published.fetch_add(1, Ordering::AcqRel);
            },
            Duration::from_millis(100),
            Duration::from_secs(1),
        );
        assert!(capture.is_err());
        let delivery = receiver.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(
            weak.upgrade().is_none(),
            "abandoned native state retained its consumer"
        );
        assert_eq!(worker::capture_count(), 1);
        // Capture and codec pools are independent, including when both codec
        // slots are needed for recovery from another native failure.
        let first = Worker::start(|| Ok(()), |_, _: ()| Ok(()), Duration::from_secs(1)).unwrap();
        let second = Worker::start(|| Ok(()), |_, _: ()| Ok(()), Duration::from_secs(1)).unwrap();
        let spare = idle_capture();
        assert_eq!(worker::capture_count(), 2);
        assert!(
            Capture::start(
                |_: Arc<Delivery<u32>>| Ok(((), info())),
                |_, _| Ok(()),
                |_| {},
                Duration::from_secs(1),
                Duration::from_secs(1)
            )
            .is_err()
        );
        delivery.emit(1);
        release.send(()).unwrap();
        finished.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(
            attempts.load(Ordering::Acquire),
            0,
            "late content started an abandoned recording"
        );
        assert_eq!(publications.load(Ordering::Acquire), 0);
        drop(spare);
        drop(first);
        drop(second);
    }

    #[test]
    fn a_stuck_update_detaches_frames_but_retains_the_native_slot_through_stop_confirmation() {
        let _serial = worker::isolated_capture();
        let (release, blocked) = mpsc::channel();
        let (entered, stopping) = mpsc::channel();
        let (exited, finished) = mpsc::channel();
        let (state, receiver) = mpsc::channel();
        struct Native(
            Arc<Delivery<u32>>,
            mpsc::Receiver<()>,
            mpsc::Sender<()>,
            mpsc::Sender<()>,
        );
        impl Drop for Native {
            fn drop(&mut self) {
                let _ = self.2.send(());
                self.0.wait_stopped();
                let _ = self.3.send(());
            }
        }
        let owner = Arc::new(());
        let weak = Arc::downgrade(&owner);
        let frames = Arc::new(AtomicUsize::new(0));
        let publications = frames.clone();
        let capture = Capture::start(
            move |delivery| {
                delivery.started(Ok(()));
                state.send(delivery.clone()).unwrap();
                Ok((Native(delivery, blocked, entered, exited), info()))
            },
            |native, fps| {
                native
                    .1
                    .recv()
                    .map_err(|_| "capture fixture input was closed".to_owned())?;
                native.0.emit(fps);
                Ok(())
            },
            move |_| {
                std::hint::black_box(&owner);
                publications.fetch_add(1, Ordering::AcqRel);
            },
            Duration::from_secs(1),
            Duration::from_millis(100),
        )
        .unwrap();
        let delivery = receiver.recv_timeout(Duration::from_secs(1)).unwrap();
        capture.set_fps(20);
        let error = capture.error().expect("configuration timeout was hidden");
        assert!(error.contains("采集"));
        assert!(weak.upgrade().is_none());
        let now = Instant::now();
        capture.stop();
        drop(capture);
        assert!(now.elapsed() < Duration::from_millis(100));
        assert_eq!(worker::capture_count(), 1);
        release.send(()).unwrap();
        stopping.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(
            worker::capture_count(),
            1,
            "cleanup released its slot before native stop"
        );
        delivery.fail("later stop error".into());
        assert_eq!(delivery.error().as_deref(), Some(error.as_str()));
        assert!(
            !delivery.is_stopped(),
            "an error is not a native stop confirmation"
        );
        delivery.stopped(None);
        finished.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(frames.load(Ordering::Acquire), 0);
    }

    #[test]
    fn delegate_stop_and_failed_start_cannot_be_undone_by_a_late_success() {
        let frames = Arc::new(AtomicUsize::new(0));
        let received = frames.clone();
        let delivery = Delivery::new(Arc::new(move |_: ()| {
            received.fetch_add(1, Ordering::AcqRel);
        }));
        delivery.stopped(Some("system stopped the stream".into()));
        delivery.started(Ok(()));
        delivery.emit(());
        assert!(delivery.check_active().is_err());
        assert!(delivery.is_stopped());
        assert_eq!(frames.load(Ordering::Acquire), 0);
        let failed = Delivery::new(Arc::new(|_: ()| {}));
        failed.started(Err("permission denied".into()));
        failed.started(Ok(()));
        assert_eq!(failed.wait_started().unwrap_err(), "permission denied");
        assert!(failed.is_stopped());
    }

    #[test]
    fn retirement_does_not_wait_for_an_inflight_consumer_and_final_guard_discards_its_picture() {
        use super::super::{EncodedFrame, publication};
        let (entered, observing) = mpsc::channel();
        let (release, blocked) = mpsc::channel();
        let blocked = Mutex::new(blocked);
        let stop = Arc::new(AtomicBool::new(false));
        let error = Arc::new(Mutex::new(None));
        let frames = Arc::new(AtomicUsize::new(0));
        let received = frames.clone();
        let sink = publication::guard(
            Arc::new(move |_| {
                received.fetch_add(1, Ordering::AcqRel);
            }),
            stop.clone(),
            error.clone(),
            || Ok(()),
        );
        let closing = stop.clone();
        let report = error.clone();
        let delivery = Arc::new(Delivery::with_hook(
            Arc::new(move |frame| {
                entered.send(()).unwrap();
                blocked.lock().unwrap().recv().unwrap();
                sink(frame);
            }),
            Arc::new(move |reason| {
                closing.store(true, Ordering::Release);
                if let Some(reason) = reason {
                    report.lock().unwrap().get_or_insert(reason);
                }
            }),
            Arc::new(|| true),
        ));
        let input = delivery.clone();
        let worker = std::thread::spawn(move || {
            input.emit(EncodedFrame {
                cursor_embedded: None,
                data: vec![0],
                keyframe: true,
                captured: Instant::now(),
                latency: Duration::ZERO,
            })
        });
        observing.recv_timeout(Duration::from_secs(1)).unwrap();
        let now = Instant::now();
        delivery.stopped(Some("source grant ended".into()));
        assert!(now.elapsed() < Duration::from_millis(100));
        assert!(
            stop.load(Ordering::Acquire),
            "delegate stop did not fence codec publication"
        );
        assert_eq!(error.lock().unwrap().as_deref(), Some("source grant ended"));
        release.send(()).unwrap();
        worker.join().unwrap();
        assert_eq!(frames.load(Ordering::Acquire), 0);
    }

    #[test]
    fn parent_lease_cancellation_during_content_preparation_prevents_the_native_start_stage() {
        let _serial = worker::isolated_capture();
        let parent = Arc::new(AtomicBool::new(false));
        let cancelled = parent.clone();
        let closed = Arc::new(AtomicBool::new(false));
        let stopped = closed.clone();
        let attempts = Arc::new(AtomicUsize::new(0));
        let starts = attempts.clone();
        let (entered, preparing) = mpsc::channel();
        let (release, blocked) = mpsc::channel();
        let owner = std::thread::spawn(move || {
            Capture::start_with_hook(
                move |delivery: Arc<Delivery<u32>>| {
                    delivery.after_prepare(
                        || {
                            entered.send(()).unwrap();
                            blocked.recv().unwrap();
                            Ok(())
                        },
                        |_| {
                            starts.fetch_add(1, Ordering::AcqRel);
                            Ok(((), info()))
                        },
                    )
                },
                |_, _| Ok(()),
                |_| {},
                move |_| {
                    stopped.store(true, Ordering::Release);
                },
                Arc::new(move || !cancelled.load(Ordering::Acquire)),
                Duration::from_secs(1),
                Duration::from_secs(1),
            )
        });
        preparing.recv_timeout(Duration::from_secs(1)).unwrap();
        parent.store(true, Ordering::Release);
        release.send(()).unwrap();
        assert!(owner.join().unwrap().is_err());
        assert_eq!(attempts.load(Ordering::Acquire), 0);
        assert!(closed.load(Ordering::Acquire));
    }
}
