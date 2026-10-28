//! Bounded, thread-owned native components with separate capture/codec pools.
//! Native interfaces stay on their worker. Codec results expire with their
//! request; continuous capture independently revokes its frame consumer.
use super::Result;
use std::{
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
static WORKERS: AtomicUsize = AtomicUsize::new(0);
#[cfg(any(target_os = "macos", target_os = "windows", test))]
static CAPTURES: AtomicUsize = AtomicUsize::new(0);
struct Availability {
    lock: Mutex<()>,
    returned: Condvar,
}
impl Availability {
    const fn new() -> Self {
        Self {
            lock: Mutex::new(()),
            returned: Condvar::new(),
        }
    }
}
static CODEC_AVAILABLE: Availability = Availability::new();
#[cfg(any(target_os = "macos", target_os = "windows", test))]
static CAPTURE_AVAILABLE: Availability = Availability::new();
#[derive(Clone, Copy)]
enum Kind {
    Codec,
    #[cfg(any(target_os = "macos", target_os = "windows", test))]
    Capture,
}
impl Kind {
    fn available(self) -> &'static Availability {
        match self {
            Self::Codec => &CODEC_AVAILABLE,
            #[cfg(any(target_os = "macos", target_os = "windows", test))]
            Self::Capture => &CAPTURE_AVAILABLE,
        }
    }
    #[cfg(test)]
    fn probe(self) -> &'static std::sync::Mutex<Option<mpsc::Sender<std::thread::ThreadId>>> {
        static CODEC: std::sync::Mutex<Option<mpsc::Sender<std::thread::ThreadId>>> =
            std::sync::Mutex::new(None);
        static CAPTURE: std::sync::Mutex<Option<mpsc::Sender<std::thread::ThreadId>>> =
            std::sync::Mutex::new(None);
        match self {
            Self::Codec => &CODEC,
            Self::Capture => &CAPTURE,
        }
    }
    #[cfg(test)]
    fn report_full(self) {
        if let Some(probe) = self.probe().lock().unwrap().as_ref() {
            let _ = probe.send(std::thread::current().id());
        }
    }
    fn count(self) -> &'static AtomicUsize {
        match self {
            Self::Codec => &WORKERS,
            #[cfg(any(target_os = "macos", target_os = "windows", test))]
            Self::Capture => &CAPTURES,
        }
    }
    fn action(self) -> &'static str {
        match self {
            Self::Codec => "编码",
            #[cfg(any(target_os = "macos", target_os = "windows", test))]
            Self::Capture => "采集",
        }
    }
    fn thread(self) -> &'static str {
        match self {
            Self::Codec => "native-video-encoder",
            #[cfg(any(target_os = "macos", target_os = "windows", test))]
            Self::Capture => "native-video-capture",
        }
    }
}
#[cfg(test)]
pub(super) static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
#[cfg(test)]
pub(super) fn isolated_capture() -> std::sync::MutexGuard<'static, ()> {
    static CAPTURE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let guard = CAPTURE_LOCK.lock().unwrap();
    for _ in 0..1000 {
        if CAPTURES.load(Ordering::Acquire) == 0 {
            return guard;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    panic!("previous capture fixture did not exit");
}
#[cfg(test)]
pub(super) fn capture_count() -> usize {
    CAPTURES.load(Ordering::Acquire)
}
#[cfg(test)]
pub(super) fn isolated() -> std::sync::MutexGuard<'static, ()> {
    let guard = TEST_LOCK.lock().unwrap();
    // Native destruction happens on the old worker after its owner is gone.
    // A new fixture starts only after the previous fixture returned its permit.
    for _ in 0..1000 {
        if WORKERS.load(Ordering::Acquire) == 0 {
            return guard;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    panic!("previous native fixture did not exit");
}
struct Permit(Kind);
impl Permit {
    fn take(kind: Kind, deadline: Instant) -> Result<Self> {
        // The existing native-operation limit also bounds retirement grace.
        // Quota waiting is part of the caller's original initialization budget.
        let until = deadline.min(Instant::now() + Duration::from_secs(2));
        let available = kind.available();
        let mut guard = available.lock.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if Instant::now() >= deadline {
                break;
            }
            if kind
                .count()
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                    (n < 2).then_some(n + 1)
                })
                .is_ok()
            {
                return Ok(Self(kind));
            }
            #[cfg(test)]
            kind.report_full();
            let remaining = until.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            let (waiting, _) = available
                .returned
                .wait_timeout(guard, remaining)
                .unwrap_or_else(|e| e.into_inner());
            guard = waiting;
        }
        Err(format!("旧系统{}器仍在退出", kind.action()))
    }
}
impl Drop for Permit {
    fn drop(&mut self) {
        let available = self.0.available();
        let _guard = available.lock.lock().unwrap_or_else(|e| e.into_inner());
        self.0.count().fetch_sub(1, Ordering::AcqRel);
        available.returned.notify_all();
    }
}
type Request<I, O> = (I, mpsc::SyncSender<Result<O>>);
pub(super) struct Worker<I, O> {
    sender: Option<mpsc::SyncSender<Request<I, O>>>,
    live: Arc<AtomicBool>,
    timeout: Duration,
    kind: Kind,
}
impl<I: Send + 'static, O: Send + 'static> Worker<I, O> {
    #[cfg(any(target_os = "windows", test))]
    pub fn start<C: 'static>(
        initialize: impl FnOnce() -> Result<C> + Send + 'static,
        process: impl FnMut(&mut C, I) -> Result<O> + Send + 'static,
        timeout: Duration,
    ) -> Result<Self> {
        Self::start_with_deadlines(initialize, process, timeout, timeout)
    }

    pub fn start_with_deadlines<C: 'static>(
        initialize: impl FnOnce() -> Result<C> + Send + 'static,
        process: impl FnMut(&mut C, I) -> Result<O> + Send + 'static,
        initialize_timeout: Duration,
        timeout: Duration,
    ) -> Result<Self> {
        Self::start_in(
            Kind::Codec,
            initialize,
            process,
            initialize_timeout,
            timeout,
        )
    }

    #[cfg(any(target_os = "macos", target_os = "windows", test))]
    pub fn start_capture<C: 'static>(
        initialize: impl FnOnce() -> Result<C> + Send + 'static,
        process: impl FnMut(&mut C, I) -> Result<O> + Send + 'static,
        initialize_timeout: Duration,
        timeout: Duration,
    ) -> Result<Self> {
        Self::start_in(
            Kind::Capture,
            initialize,
            process,
            initialize_timeout,
            timeout,
        )
    }

    fn start_in<C: 'static>(
        kind: Kind,
        initialize: impl FnOnce() -> Result<C> + Send + 'static,
        mut process: impl FnMut(&mut C, I) -> Result<O> + Send + 'static,
        initialize_timeout: Duration,
        timeout: Duration,
    ) -> Result<Self> {
        let deadline = Instant::now()
            .checked_add(initialize_timeout)
            .ok_or_else(|| format!("系统{}器初始化期限无效", kind.action()))?;
        let permit = Permit::take(kind, deadline)?;
        let (sender, receiver) = mpsc::sync_channel::<Request<I, O>>(1);
        let (ready, initialized) = mpsc::sync_channel(1);
        let live = Arc::new(AtomicBool::new(true));
        let active = live.clone();
        std::thread::Builder::new()
            .name(kind.thread().into())
            .spawn(move || {
                let _permit = permit;
                // Scheduling delay or retirement waiting cannot start a new
                // native initialization after its original owner has expired.
                if !active.load(Ordering::Acquire) || Instant::now() >= deadline {
                    let _ = ready.send(Err(format!("系统{}器初始化未获确认", kind.action())));
                    return;
                }
                // C may be !Send (COM, GL, ...). It is created, used and dropped on
                // this thread, including initialization failure and abandoned ready.
                let mut codec = match initialize() {
                    Ok(codec) => codec,
                    Err(error) => {
                        let _ = ready.send(Err(error));
                        return;
                    }
                };
                if !active.load(Ordering::Acquire)
                    || Instant::now() >= deadline
                    || ready.send(Ok(())).is_err()
                {
                    return;
                }
                while let Ok((input, reply)) = receiver.recv() {
                    if !active.load(Ordering::Acquire) {
                        break;
                    }
                    let result = process(&mut codec, input);
                    let failed = result.is_err();
                    if !active.load(Ordering::Acquire) {
                        break;
                    }
                    if reply.send(result).is_err() || failed {
                        break;
                    }
                }
            })
            .map_err(|_| format!("无法启动系统{}线程", kind.action()))?;
        let mut worker = Self {
            sender: Some(sender),
            live,
            timeout,
            kind,
        };
        match initialized.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(Ok(())) => Ok(worker),
            Ok(Err(error)) => {
                worker.retire();
                Err(error)
            }
            Err(_) => {
                worker.retire();
                Err(format!("系统{}器初始化未获确认", kind.action()))
            }
        }
    }
    pub fn run(&mut self, input: I) -> Result<O> {
        self.run_with_timeout(input, self.timeout)
    }
    pub fn run_with_timeout(&mut self, input: I, budget: Duration) -> Result<O> {
        let timeout = self.timeout.min(budget);
        if timeout.is_zero() {
            self.retire();
            return Err(format!("系统{}请求已过期", self.kind.action()));
        }
        let (reply, response) = mpsc::sync_channel(1);
        let sent = self
            .sender
            .as_ref()
            .is_some_and(|sender| sender.try_send((input, reply)).is_ok());
        if !sent {
            self.retire();
            return Err(format!("系统{}线程不可用", self.kind.action()));
        }
        match response.recv_timeout(timeout) {
            Ok(Ok(output)) => Ok(output),
            Ok(Err(error)) => {
                self.retire();
                Err(error)
            }
            Err(_) => {
                self.retire();
                Err(format!("系统{}未及时返回", self.kind.action()))
            }
        }
    }
    fn retire(&mut self) {
        self.live.store(false, Ordering::Release);
        self.sender.take();
    }
}
impl<I, O> Drop for Worker<I, O> {
    fn drop(&mut self) {
        self.live.store(false, Ordering::Release);
        self.sender.take();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn an_expired_budget_has_no_native_effect_and_a_short_request_does_not_get_the_full_timeout() {
        let _serial = isolated();
        let calls = Arc::new(AtomicUsize::new(0));
        let called = calls.clone();
        let mut expired = Worker::start(
            || Ok(()),
            move |_, ()| {
                called.fetch_add(1, Ordering::AcqRel);
                Ok(())
            },
            Duration::from_secs(1),
        )
        .unwrap();
        assert!(expired.run_with_timeout((), Duration::ZERO).is_err());
        assert_eq!(calls.load(Ordering::Acquire), 0);
        drop(expired);
        let (release, blocked) = mpsc::channel();
        let mut worker = Worker::start(
            move || Ok(blocked),
            |native, ()| {
                native.recv().map_err(|_| "test release closed")?;
                Ok(())
            },
            Duration::from_secs(2),
        )
        .unwrap();
        let started = Instant::now();
        let result = worker.run_with_timeout((), Duration::from_millis(40));
        let elapsed = started.elapsed();
        drop(worker);
        let _ = release.send(());
        let until = Instant::now() + Duration::from_secs(1);
        while WORKERS.load(Ordering::Acquire) != 0 && Instant::now() < until {
            std::thread::yield_now();
        }
        assert_eq!(WORKERS.load(Ordering::Acquire), 0);
        assert!(result.is_err());
        assert!(elapsed < Duration::from_millis(500));
    }
    struct FullProbe(Kind, mpsc::Receiver<std::thread::ThreadId>);
    impl FullProbe {
        fn new(kind: Kind) -> Self {
            let (sent, received) = mpsc::channel();
            *kind.probe().lock().unwrap() = Some(sent);
            Self(kind, received)
        }
        fn wait_for(&self, owner: std::thread::ThreadId) {
            while self.1.recv_timeout(Duration::from_secs(1)).unwrap() != owner {}
        }
    }
    impl Drop for FullProbe {
        fn drop(&mut self) {
            self.0.probe().lock().unwrap().take();
        }
    }

    fn holding(kind: Kind) -> (Worker<(), ()>, mpsc::Sender<()>, mpsc::Receiver<()>) {
        struct Native(mpsc::Sender<()>, mpsc::Receiver<()>);
        impl Drop for Native {
            fn drop(&mut self) {
                let _ = self.0.send(());
                let _ = self.1.recv();
            }
        }
        let (release, blocked) = mpsc::channel();
        let (entered, destroying) = mpsc::channel();
        let worker = Worker::start_in(
            kind,
            move || Ok(Native(entered, blocked)),
            |_, _: ()| Ok(()),
            Duration::from_secs(1),
            Duration::from_secs(1),
        )
        .unwrap();
        (worker, release, destroying)
    }

    #[test]
    fn a_retiring_slot_can_be_reused_while_another_native_context_is_stuck() {
        let _captures = isolated_capture();
        let _codecs = isolated();
        for kind in [Kind::Codec, Kind::Capture] {
            let (stuck, release_stuck, stuck_drop) = holding(kind);
            let (retiring, release_retiring, retiring_drop) = holding(kind);
            drop(stuck);
            stuck_drop.recv_timeout(Duration::from_secs(1)).unwrap();
            drop(retiring);
            retiring_drop.recv_timeout(Duration::from_secs(1)).unwrap();
            let probe = FullProbe::new(kind);
            let starts = Arc::new(AtomicUsize::new(0));
            let initializing = starts.clone();
            let owner = std::thread::spawn(move || {
                Worker::start_in(
                    kind,
                    move || {
                        initializing.fetch_add(1, Ordering::AcqRel);
                        Ok(())
                    },
                    |_, _: ()| Ok(()),
                    Duration::from_secs(1),
                    Duration::from_secs(1),
                )
            });
            // The acquisition path itself reports full. No timing sleep is
            // used to guess whether the replacement has reached the pool.
            probe.wait_for(owner.thread().id());
            assert_eq!(kind.count().load(Ordering::Acquire), 2);
            assert_eq!(starts.load(Ordering::Acquire), 0);
            release_retiring.send(()).unwrap();
            let next = owner.join().unwrap();
            release_stuck.send(()).unwrap();
            assert!(
                next.is_ok(),
                "{} replacement failed before retiring cleanup returned its slot",
                kind.action()
            );
            assert_eq!(starts.load(Ordering::Acquire), 1);
            drop(next);
            drop(probe);
            for _ in 0..1000 {
                if kind.count().load(Ordering::Acquire) == 0 {
                    break;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            assert_eq!(kind.count().load(Ordering::Acquire), 0);
        }
    }

    #[test]
    fn full_pools_never_start_a_third_context_and_wait_only_within_the_budget() {
        let _captures = isolated_capture();
        let _codecs = isolated();
        for kind in [Kind::Codec, Kind::Capture] {
            let (first, release_first, first_drop) = holding(kind);
            let (second, release_second, second_drop) = holding(kind);
            drop(first);
            first_drop.recv_timeout(Duration::from_secs(1)).unwrap();
            drop(second);
            second_drop.recv_timeout(Duration::from_secs(1)).unwrap();
            let starts = Arc::new(AtomicUsize::new(0));
            for (budget, lower, upper) in [
                (
                    Duration::from_millis(60),
                    Duration::from_millis(40),
                    Duration::from_millis(500),
                ),
                (
                    Duration::from_secs(10),
                    Duration::from_millis(1800),
                    Duration::from_secs(3),
                ),
            ] {
                let initialized = starts.clone();
                let now = Instant::now();
                let result = Worker::start_in(
                    kind,
                    move || {
                        initialized.fetch_add(1, Ordering::AcqRel);
                        Ok(())
                    },
                    |_, _: ()| Ok(()),
                    budget,
                    Duration::from_secs(1),
                );
                let elapsed = now.elapsed();
                assert!(result.is_err());
                assert!(
                    elapsed >= lower && elapsed < upper,
                    "{} quota wait: {elapsed:?}",
                    kind.action()
                );
                assert_eq!(kind.count().load(Ordering::Acquire), 2);
                assert_eq!(starts.load(Ordering::Acquire), 0);
            }
            release_first.send(()).unwrap();
            release_second.send(()).unwrap();
            for _ in 0..1000 {
                if kind.count().load(Ordering::Acquire) == 0 {
                    break;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            assert_eq!(kind.count().load(Ordering::Acquire), 0);
        }
    }

    #[test]
    fn quota_wait_does_not_reset_the_initialization_deadline() {
        let _captures = isolated_capture();
        let _codecs = isolated();
        for kind in [Kind::Codec, Kind::Capture] {
            let (stuck, release_stuck, stuck_drop) = holding(kind);
            let (retiring, release_retiring, retiring_drop) = holding(kind);
            drop(stuck);
            stuck_drop.recv_timeout(Duration::from_secs(1)).unwrap();
            drop(retiring);
            retiring_drop.recv_timeout(Duration::from_secs(1)).unwrap();
            let probe = FullProbe::new(kind);
            let (entered, initializing) = mpsc::channel();
            let (release_init, blocked_init) = mpsc::channel();
            let owner = std::thread::spawn(move || {
                let now = Instant::now();
                let result = Worker::start_in(
                    kind,
                    move || {
                        entered.send(()).unwrap();
                        let _ = blocked_init.recv();
                        Ok(())
                    },
                    |_, _: ()| Ok(()),
                    Duration::from_secs(1),
                    Duration::from_secs(1),
                );
                (result, now.elapsed())
            });
            probe.wait_for(owner.thread().id());
            // A deliberate part of the one-second budget is consumed by the
            // old context. The new initializer then stays blocked until timeout.
            std::thread::sleep(Duration::from_millis(600));
            release_retiring.send(()).unwrap();
            initializing.recv_timeout(Duration::from_secs(1)).unwrap();
            let (result, elapsed) = owner.join().unwrap();
            release_init.send(()).unwrap();
            release_stuck.send(()).unwrap();
            assert!(result.is_err());
            assert!(
                elapsed < Duration::from_millis(1300),
                "{} reset the startup budget: {elapsed:?}",
                kind.action()
            );
            drop(probe);
            for _ in 0..1000 {
                if kind.count().load(Ordering::Acquire) == 0 {
                    break;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            assert_eq!(kind.count().load(Ordering::Acquire), 0);
        }
    }

    #[test]
    fn rapid_rebuilds_reuse_the_one_healthy_slot_without_lost_wakeups() {
        let _captures = isolated_capture();
        let _codecs = isolated();
        for kind in [Kind::Codec, Kind::Capture] {
            let (stuck, release_stuck, stuck_drop) = holding(kind);
            drop(stuck);
            stuck_drop.recv_timeout(Duration::from_secs(1)).unwrap();
            let mut current = holding(kind);
            let probe = FullProbe::new(kind);
            for _ in 0..24 {
                let (old, release_old, old_drop) = current;
                drop(old);
                old_drop.recv_timeout(Duration::from_secs(1)).unwrap();
                let owner = std::thread::spawn(move || holding(kind));
                probe.wait_for(owner.thread().id());
                assert_eq!(kind.count().load(Ordering::Acquire), 2);
                release_old.send(()).unwrap();
                current = owner.join().unwrap();
                current.0.run(()).unwrap();
                assert_eq!(kind.count().load(Ordering::Acquire), 2);
            }
            drop(probe);
            let (last, release_last, last_drop) = current;
            drop(last);
            last_drop.recv_timeout(Duration::from_secs(1)).unwrap();
            release_last.send(()).unwrap();
            release_stuck.send(()).unwrap();
            for _ in 0..1000 {
                if kind.count().load(Ordering::Acquire) == 0 {
                    break;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            assert_eq!(kind.count().load(Ordering::Acquire), 0);
        }
    }
    fn idle() {
        for _ in 0..1000 {
            if WORKERS.load(Ordering::Acquire) == 0 {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        panic!("native worker did not exit");
    }
    #[test]
    fn cold_initialization_does_not_relax_the_running_request_deadline() {
        let _serial = isolated();
        let (release, blocked) = mpsc::channel();
        let mut worker = Worker::start_with_deadlines(
            || {
                std::thread::sleep(Duration::from_millis(80));
                Ok(())
            },
            move |_, _: ()| {
                blocked.recv().unwrap();
                Ok(42)
            },
            Duration::from_secs(1),
            Duration::from_millis(30),
        )
        .unwrap();
        let now = std::time::Instant::now();
        assert!(worker.run(()).is_err());
        assert!(now.elapsed() < Duration::from_millis(500));
        drop(worker);
        release.send(()).unwrap();
        idle();
    }

    #[test]
    fn non_send_native_state_is_created_used_and_dropped_on_its_worker_thread() {
        let _serial = isolated();
        idle();
        let owner = std::thread::current().id();
        let dropped = Arc::new(AtomicBool::new(false));
        let observed = dropped.clone();
        struct Native(std::rc::Rc<usize>, std::thread::ThreadId, Arc<AtomicBool>);
        impl Drop for Native {
            fn drop(&mut self) {
                assert_eq!(self.1, std::thread::current().id());
                self.2.store(true, Ordering::Release);
            }
        }
        let mut worker = Worker::start(
            move || {
                assert_ne!(owner, std::thread::current().id());
                Ok(Native(
                    std::rc::Rc::new(7),
                    std::thread::current().id(),
                    observed,
                ))
            },
            |native, n| {
                assert_eq!(native.1, std::thread::current().id());
                Ok(*native.0 + n)
            },
            Duration::from_secs(1),
        )
        .unwrap();
        assert_eq!(worker.run(4).unwrap(), 11);
        drop(worker);
        idle();
        assert!(dropped.load(Ordering::Acquire));
    }
    #[test]
    fn timed_out_output_is_abandoned_and_close_does_not_join_a_blocked_os_call() {
        let _serial = isolated();
        idle();
        let (release, blocked) = mpsc::channel();
        let mut worker = Worker::start(
            || Ok(()),
            move |_, _: ()| {
                blocked.recv().unwrap();
                Ok(42)
            },
            Duration::from_secs(1),
        )
        .unwrap();
        worker.timeout = Duration::from_millis(30);
        assert!(worker.run(()).is_err());
        let now = std::time::Instant::now();
        drop(worker);
        assert!(now.elapsed() < Duration::from_millis(100));
        assert_eq!(WORKERS.load(Ordering::Acquire), 1);
        release.send(()).unwrap();
        idle();
    }
    #[test]
    fn abandoned_initialization_retains_its_slot_and_reconnects_cannot_leak_unbounded_threads() {
        let _serial = isolated();
        idle();
        let mut releases = vec![];
        for _ in 0..2 {
            let (release, blocked) = mpsc::channel();
            releases.push(release);
            let worker = Worker::start(
                move || {
                    blocked.recv().unwrap();
                    Ok(())
                },
                |_, _: ()| Ok(()),
                Duration::from_millis(30),
            );
            assert!(worker.is_err());
        }
        assert_eq!(WORKERS.load(Ordering::Acquire), 2);
        assert!(Worker::start(|| Ok(()), |_, _: ()| Ok(()), Duration::from_millis(30)).is_err());
        for release in releases {
            release.send(()).unwrap();
        }
        idle();
    }
}
