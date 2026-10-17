//! Startup work belongs to the service, without blocking its accept loop.
use std::{
    future::Future,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

#[derive(Default)]
pub(super) struct Work(Arc<AtomicUsize>);
struct Running(Arc<AtomicUsize>);
impl Drop for Running {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}
impl Work {
    pub fn active(&self) -> bool {
        self.0.load(Ordering::Acquire) != 0
    }
    pub fn spawn<T: Send + 'static>(
        &self,
        work: impl Future<Output = T> + Send + 'static,
    ) -> tokio::task::JoinHandle<T> {
        // Register before spawning: an idle request can arrive before the task
        // gets its first poll. Drop releases the marker on error/panic/cancel.
        self.0.fetch_add(1, Ordering::AcqRel);
        let running = Running(self.0.clone());
        tokio::spawn(async move {
            let _running = running;
            work.await
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn startup_work_is_owned_before_first_poll_and_releases_on_every_exit() {
        let work = Work::default();
        let cancelled = work.spawn(std::future::pending::<()>());
        assert!(work.active());
        let finished = work.spawn(async { Err::<(), _>("expected failure") });
        assert!(finished.await.unwrap().is_err());
        assert!(work.active());
        cancelled.abort();
        assert!(cancelled.await.unwrap_err().is_cancelled());
        assert!(!work.active());
        let panicked = work.spawn(async { panic!("isolated startup fixture panic") });
        assert!(panicked.await.unwrap_err().is_panic());
        assert!(!work.active());
        work.spawn(async {}).await.unwrap();
        assert!(!work.active());
    }
}
