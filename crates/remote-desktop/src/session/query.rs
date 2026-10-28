//! A single bounded OS-query worker; replies belong to one caller and expire.
use crate::Result;
use std::{
    sync::mpsc,
    time::{Duration, Instant},
};
const MAX_PENDING: usize = 32;

struct Request {
    deadline: Instant,
    reply: mpsc::SyncSender<Result<bool>>,
}
pub(super) struct Probe {
    sender: mpsc::SyncSender<Request>,
    budget: Duration,
}
impl Probe {
    pub fn start(
        mut query: impl FnMut() -> Result<bool> + Send + 'static,
        budget: Duration,
    ) -> Result<Self> {
        let (sender, receiver) = mpsc::sync_channel::<Request>(MAX_PENDING);
        std::thread::Builder::new()
            .name("desktop-session-query".into())
            .spawn(move || {
                while let Ok(request) = receiver.recv() {
                    if Instant::now() >= request.deadline {
                        continue;
                    }
                    let result = query();
                    // An old unlocked answer never becomes the next caller's grant.
                    if Instant::now() < request.deadline {
                        let _ = request.reply.send(result);
                    }
                }
            })
            .map_err(|_| "无法启动桌面状态检查")?;
        Ok(Self { sender, budget })
    }
    pub fn read(&self) -> Result<bool> {
        let deadline = Instant::now() + self.budget;
        let (reply, response) = mpsc::sync_channel(1);
        self.sender
            .try_send(Request { deadline, reply })
            .map_err(|_| "桌面状态检查繁忙，请重新连接")?;
        let result = response
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .map_err(|_| "检查系统桌面状态超时")?;
        if Instant::now() >= deadline {
            return Err("检查系统桌面状态超时".into());
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    #[test]
    fn fresh_queries_do_not_cache_an_unlocked_grant_or_hide_errors() {
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        let probe = Probe::start(
            move || match seen.fetch_add(1, Ordering::SeqCst) {
                0 => Ok(false),
                1 => Ok(true),
                _ => Err("unknown session".into()),
            },
            Duration::from_secs(1),
        )
        .unwrap();
        assert!(!probe.read().unwrap());
        assert!(probe.read().unwrap());
        assert!(probe.read().is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }
    #[test]
    fn stalled_query_has_bounded_queue_and_its_late_unlocked_answer_is_retired() {
        let (entered, started) = mpsc::sync_channel(1);
        let (release, wait) = mpsc::sync_channel(1);
        let mut first = true;
        let probe = Arc::new(
            Probe::start(
                move || {
                    if first {
                        first = false;
                        entered.send(()).unwrap();
                        wait.recv().unwrap();
                        Ok(false)
                    } else {
                        Ok(true)
                    }
                },
                Duration::from_millis(80),
            )
            .unwrap(),
        );
        let sending = probe.clone();
        let first = std::thread::spawn(move || sending.read());
        started.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(first.join().unwrap().is_err());
        // The worker is still occupied. A queued caller expires; fill the
        // remaining finite slots without creating more threads or waiting.
        assert!(probe.read().is_err());
        for _ in 1..MAX_PENDING {
            let (reply, _) = mpsc::sync_channel(1);
            assert!(
                probe
                    .sender
                    .try_send(Request {
                        deadline: Instant::now(),
                        reply
                    })
                    .is_ok()
            );
        }
        assert!(probe.read().unwrap_err().contains("繁忙"));
        release.send(()).unwrap();
        let until = Instant::now() + Duration::from_secs(1);
        loop {
            match probe.read() {
                Ok(locked) => {
                    assert!(locked);
                    break;
                }
                Err(_) if Instant::now() < until => std::thread::yield_now(),
                Err(e) => panic!("{e}"),
            }
        }
    }
}
