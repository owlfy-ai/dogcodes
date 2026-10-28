//! Recover inside the original desktop lease; never select a new portal source.
use super::{
    Arc, Duration, Instant, Mutex, Output, Publication, Resize, ResizeRequest, Stats, Value,
    VideoOut, VideoProfile, json,
};
use std::collections::VecDeque;
use std::sync::atomic::Ordering;
const DEADLINE: Duration = Duration::from_secs(20);
const WINDOW: Duration = Duration::from_secs(30);
const ATTEMPTS: usize = 3;
#[derive(Default)]
struct State {
    attempts: VecDeque<Instant>,
    total: u64,
    completed: u64,
    began: Option<Instant>,
    waiting: bool,
}
#[derive(Default)]
pub(super) struct Recovery(Mutex<State>);
impl Recovery {
    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    fn begin(&self) {
        let mut s = self.state();
        // A pipeline which fails again before its first accepted IDR belongs
        // to the same recovery deadline, not a fresh unbounded grace period.
        s.began.get_or_insert_with(Instant::now);
        s.waiting = false;
    }
    fn attempt(&self, now: Instant) -> Result<Duration, String> {
        let mut s = self.state();
        s.attempts
            .retain(|at| now.saturating_duration_since(*at) < WINDOW);
        if s.attempts.len() >= ATTEMPTS
            || s.began
                .is_some_and(|at| now.saturating_duration_since(at) >= DEADLINE)
        {
            return Err("视频恢复未成功，已停止自动重试".into());
        }
        s.attempts.push_back(now);
        s.total += 1;
        Ok(Duration::from_millis(250 * (1 << (s.attempts.len() - 1))))
    }
    pub fn active(&self) -> bool {
        self.state().began.is_some()
    }
    pub fn observed_idr(&self) {
        let mut s = self.state();
        if s.waiting {
            s.began = None;
            s.waiting = false;
            s.completed += 1;
        }
    }
    pub fn timeout(&self) -> Option<String> {
        self.state()
            .began
            .filter(|at| at.elapsed() >= DEADLINE)
            .map(|_| "视频恢复等待超时，请重新连接".into())
    }
    pub fn describe(&self) -> Value {
        let s = self.state();
        json!({"state":if s.began.is_none(){"ready"}else if s.waiting{"warming"}else{"restarting"},
            "attempts":s.total,"completed":s.completed,"timeoutMs":DEADLINE.as_millis()})
    }
}

pub(super) struct Context<'a> {
    pub sender: &'a Arc<Output>,
    pub stats: &'a Stats,
    pub resize: &'a Resize,
    pub publication: &'a Publication,
}
pub(super) struct Opened<T> {
    pub stream: T,
    pub size: (u32, u32),
    pub limit: u32,
    pub profile: VideoProfile,
}
/// The real pipeline owner calls this after a runtime fault, never after a
/// failed initial start. The retained source/grant, display and lease stay fixed.
pub(super) fn rebuild<T>(
    original: T,
    reason: String,
    context: Context<'_>,
    mut authorize: impl FnMut() -> Result<(), String>,
    mut open: impl FnMut(ResizeRequest, u64) -> Result<Opened<T>, String>,
) -> Result<Opened<T>, String> {
    let Context {
        sender,
        stats,
        resize,
        publication,
    } = context;
    stats.recovery.begin();
    resize.begin();
    let mut epoch = publication.advance();
    // Fence callbacks before native teardown, including callbacks made by Drop.
    drop(original);
    let live = || {
        if publication.stop.load(Ordering::Acquire) || sender.closed() {
            Err("视频请求已结束".to_owned())
        } else {
            Ok(())
        }
    };
    let mut last = reason;
    loop {
        live()?;
        authorize()?;
        let delay = stats
            .recovery
            .attempt(Instant::now())
            .map_err(|limit| format!("{limit}：{last}"))?;
        // Do not sleep through lease retirement or keep spawning replacement
        // work after an output is closed. No RPC/input worker waits here.
        let until = Instant::now() + delay;
        while Instant::now() < until {
            live()?;
            std::thread::sleep(
                until
                    .saturating_duration_since(Instant::now())
                    .min(Duration::from_millis(20)),
            );
        }
        live()?;
        authorize()?;
        if let Some(error) = stats.recovery.timeout() {
            return Err(error);
        }
        sender.rebuild();
        let request = resize.latest();
        match open(request, epoch) {
            Ok(opened) => {
                live()?;
                authorize()?;
                if let Some(error) = stats.recovery.timeout() {
                    return Err(error);
                }
                // Opening native capture is not recovery success. Only the
                // first new IDR accepted after decoder readiness proves that.
                stats.recovery.state().waiting = true;
                resize.applied(request, opened.size, opened.limit, sender.video_epoch());
                sender.codec_applied(opened.profile);
                return Ok(opened);
            }
            Err(error) => {
                last = error;
                epoch = publication.advance();
            }
        }
    }
}

#[cfg(test)]
mod tests;
