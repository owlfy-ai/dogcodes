use super::*;
use async_trait::async_trait;
use driver_api::{DriverError, DriverEventStream, FakeDriver, PromptRequest, SessionRequest};
use std::sync::{
    Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use tokio::sync::Semaphore;

struct Driver {
    inner: FakeDriver,
    handle: SessionHandle,
    create: Semaphore,
    creates: AtomicUsize,
    catalog: Semaphore,
    catalogs: AtomicUsize,
    catalogs_finished: AtomicUsize,
    prompts: AtomicUsize,
    hold_prompt: AtomicBool,
    panic_prompt: AtomicBool,
    reject: AtomicBool,
    panic: AtomicBool,
    directory: Mutex<Option<std::path::PathBuf>>,
    close: Semaphore,
    closes: AtomicUsize,
    interrupts: AtomicUsize,
    close_error: AtomicBool,
    close_returned: AtomicBool,
    proof: Option<Arc<Semaphore>>,
    proof_available: AtomicBool,
    captures: AtomicUsize,
}
fn driver(proof: bool) -> Arc<Driver> {
    Arc::new(Driver {
        inner: FakeDriver::new("fake").unwrap(),
        handle: SessionHandle::scoped("temporary-original".into()),
        create: Semaphore::new(0),
        creates: AtomicUsize::new(0),
        catalog: Semaphore::new(10),
        catalogs: AtomicUsize::new(0),
        catalogs_finished: AtomicUsize::new(0),
        prompts: AtomicUsize::new(0),
        hold_prompt: AtomicBool::new(false),
        panic_prompt: AtomicBool::new(false),
        reject: AtomicBool::new(false),
        panic: AtomicBool::new(false),
        directory: Mutex::default(),
        close: Semaphore::new(0),
        closes: AtomicUsize::new(0),
        interrupts: AtomicUsize::new(0),
        close_error: AtomicBool::new(false),
        close_returned: AtomicBool::new(false),
        proof: proof.then(|| Arc::new(Semaphore::new(0))),
        proof_available: AtomicBool::new(true),
        captures: AtomicUsize::new(0),
    })
}
#[async_trait]
impl AgentDriver for Driver {
    fn executor(&self) -> &crate::ExecutorRef {
        self.inner.executor()
    }
    fn driver_version(&self) -> &'static str {
        "temporary-lifecycle-fixture"
    }
    async fn model_catalog(&self) -> Result<driver_api::ModelCatalog, DriverError> {
        self.catalogs.fetch_add(1, Ordering::AcqRel);
        self.catalog.acquire().await.unwrap().forget();
        self.catalogs_finished.fetch_add(1, Ordering::AcqRel);
        Ok(driver_api::ModelCatalog::default())
    }
    async fn create_session(&self, _: SessionRequest) -> Result<SessionHandle, DriverError> {
        panic!("must use the analysis constructor")
    }
    async fn create_analysis_session(&self, cwd: &str) -> Result<SessionHandle, DriverError> {
        assert!(std::path::Path::new(cwd).is_dir());
        *self.directory.lock().unwrap() = Some(cwd.into());
        self.creates.fetch_add(1, Ordering::AcqRel);
        self.create.acquire().await.unwrap().forget();
        assert!(!self.panic.load(Ordering::Acquire), "private startup panic");
        if self.reject.load(Ordering::Acquire) {
            return Err(DriverError::NotConfigured("private rejection".into()));
        }
        Ok(self.handle.clone())
    }
    async fn create_selected_session(
        &self,
        request: SessionRequest,
        _: Option<&str>,
        _: Option<&str>,
    ) -> Result<SessionHandle, DriverError> {
        self.create_analysis_session(&request.working_directory)
            .await
    }
    async fn prompt(
        &self,
        handle: &SessionHandle,
        request: PromptRequest,
    ) -> Result<DriverEventStream, DriverError> {
        assert!(self.handle.same_scope(handle));
        assert_eq!(
            std::path::Path::new(&request.working_directory),
            directory(self)
        );
        self.prompts.fetch_add(1, Ordering::AcqRel);
        assert!(
            !self.panic_prompt.load(Ordering::Acquire),
            "private prompt panic"
        );
        if self.hold_prompt.load(Ordering::Acquire) {
            std::future::pending::<()>().await;
        }
        Ok(Box::pin(futures::stream::iter([
            Ok(driver_api::DriverEvent::TextDelta {
                item_id: "summary".into(),
                text: "Improve remote recovery".into(),
            }),
            Ok(driver_api::DriverEvent::TurnCompleted {
                summary: String::new(),
            }),
        ])))
    }
    async fn interrupt(&self, handle: &SessionHandle) -> Result<(), DriverError> {
        assert!(self.handle.same_scope(handle));
        self.interrupts.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }
    async fn shutdown_proof(
        &self,
        handle: &SessionHandle,
    ) -> Option<driver_api::SessionShutdownProof> {
        assert!(self.handle.same_scope(handle));
        self.captures.fetch_add(1, Ordering::AcqRel);
        if !self.proof_available.load(Ordering::Acquire) {
            return None;
        }
        let proof = self.proof.clone()?;
        Some(Box::pin(async move {
            proof.acquire().await.unwrap().forget();
        }))
    }
    async fn shutdown(&self, handle: SessionHandle) -> Result<(), DriverError> {
        assert!(self.handle.same_scope(&handle));
        self.closes.fetch_add(1, Ordering::AcqRel);
        self.close.acquire().await.unwrap().forget();
        assert!(self.directory.lock().unwrap().as_ref().unwrap().is_dir());
        self.close_returned.store(true, Ordering::Release);
        if self.close_error.load(Ordering::Acquire) {
            return Err(DriverError::Disconnected("private close failure".into()));
        }
        Ok(())
    }
}
#[derive(Default)]
struct Flags {
    released: AtomicBool,
    pending: AtomicUsize,
}
struct Lifetime(Arc<Flags>);
impl AnalysisLifetime for Lifetime {
    fn cleanup_pending(&self, _: &str) {
        self.0.pending.fetch_add(1, Ordering::AcqRel);
    }
}
impl Drop for Lifetime {
    fn drop(&mut self) {
        self.0.released.store(true, Ordering::Release);
    }
}
fn lifetime() -> (Arc<Flags>, Arc<dyn AnalysisLifetime>) {
    let flags = Arc::new(Flags::default());
    (flags.clone(), Arc::new(Lifetime(flags)))
}
async fn wait_for(test: impl Fn() -> bool) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while !test() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("private auxiliary boundary was never reached");
}
async fn start(
    driver: Arc<Driver>,
    lifetime: Arc<dyn AnalysisLifetime>,
    budget: Duration,
) -> Result<Session, String> {
    Session::start_with_budget(
        driver,
        Some(lifetime),
        Purpose::Analysis,
        |driver, cwd, _| async move {
            driver
                .create_analysis_session(&cwd)
                .await
                .map_err(|e| e.to_string())
        },
        budget,
    )
    .await
}
fn directory(driver: &Driver) -> std::path::PathBuf {
    driver.directory.lock().unwrap().clone().unwrap()
}

#[tokio::test]
async fn startup_timeout_keeps_the_original_future_assets_and_late_session_cleanup() {
    let driver = driver(false);
    let (flags, lifetime) = lifetime();
    assert!(
        start(driver.clone(), lifetime, Duration::from_millis(20))
            .await
            .err()
            .unwrap()
            .contains("启动尚未确认")
    );
    assert_eq!(driver.creates.load(Ordering::Acquire), 1);
    assert_eq!(flags.pending.load(Ordering::Acquire), 1);
    assert!(!flags.released.load(Ordering::Acquire));
    assert!(directory(&driver).is_dir());
    driver.create.add_permits(1);
    wait_for(|| driver.closes.load(Ordering::Acquire) == 1).await;
    assert!(!flags.released.load(Ordering::Acquire));
    driver.close.add_permits(1);
    wait_for(|| flags.released.load(Ordering::Acquire)).await;
    assert!(!directory(&driver).exists());
    assert_eq!(driver.creates.load(Ordering::Acquire), 1);
    assert_eq!(driver.interrupts.load(Ordering::Acquire), 1);
    assert_eq!(driver.closes.load(Ordering::Acquire), 1);
    assert_eq!(driver.prompts.load(Ordering::Acquire), 0);
}

#[tokio::test]
async fn abandoning_startup_cannot_drop_its_directory_or_replay_native_creation() {
    let driver = driver(false);
    let (flags, lifetime) = lifetime();
    let source = driver.clone();
    let waiter =
        tokio::spawn(async move { start(source, lifetime, Duration::from_secs(30)).await });
    wait_for(|| driver.creates.load(Ordering::Acquire) == 1).await;
    waiter.abort();
    assert!(waiter.await.err().unwrap().is_cancelled());
    assert_eq!(flags.pending.load(Ordering::Acquire), 1);
    assert!(!flags.released.load(Ordering::Acquire));
    driver.create.add_permits(1);
    wait_for(|| driver.closes.load(Ordering::Acquire) == 1).await;
    driver.close.add_permits(1);
    wait_for(|| flags.released.load(Ordering::Acquire)).await;
    assert_eq!(driver.creates.load(Ordering::Acquire), 1);
    assert_eq!(driver.closes.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn a_late_explicit_startup_rejection_releases_assets_without_inventing_a_session() {
    let driver = driver(false);
    driver.reject.store(true, Ordering::Release);
    let (flags, lifetime) = lifetime();
    assert!(
        start(driver.clone(), lifetime, Duration::from_millis(20))
            .await
            .is_err()
    );
    assert!(!flags.released.load(Ordering::Acquire));
    driver.create.add_permits(1);
    wait_for(|| flags.released.load(Ordering::Acquire)).await;
    assert!(!directory(&driver).exists());
    assert_eq!(driver.closes.load(Ordering::Acquire), 0);
    assert_eq!(driver.interrupts.load(Ordering::Acquire), 0);
}

#[tokio::test]
async fn startup_panic_without_an_identity_keeps_assets_unknown_instead_of_claiming_exit() {
    let driver = driver(false);
    driver.panic.store(true, Ordering::Release);
    driver.create.add_permits(1);
    let (flags, lifetime) = lifetime();
    assert!(
        start(driver.clone(), lifetime, Duration::from_secs(3))
            .await
            .err()
            .unwrap()
            .contains("启动尚未确认")
    );
    assert_eq!(flags.pending.load(Ordering::Acquire), 1);
    assert!(!flags.released.load(Ordering::Acquire));
    assert!(directory(&driver).is_dir());
    assert_eq!(driver.closes.load(Ordering::Acquire), 0);
}

#[tokio::test]
async fn cleanup_uses_the_previously_captured_proof_and_keeps_assets_after_observer_loss() {
    let driver = driver(true);
    driver.create.add_permits(1);
    driver.close.add_permits(1);
    driver.close_error.store(true, Ordering::Release);
    let (flags, lifetime) = lifetime();
    let mut session = start(driver.clone(), lifetime, Duration::from_secs(3))
        .await
        .unwrap();
    driver.proof_available.store(false, Ordering::Release);
    let cleanup = session.cleanup(false).unwrap();
    assert!(cleanup.await.unwrap().unwrap_err().contains("关闭尚未确认"));
    drop(session);
    assert_eq!(flags.pending.load(Ordering::Acquire), 1);
    assert!(!flags.released.load(Ordering::Acquire));
    assert!(directory(&driver).is_dir());
    driver.proof.as_ref().unwrap().add_permits(1);
    wait_for(|| flags.released.load(Ordering::Acquire)).await;
    assert_eq!(driver.closes.load(Ordering::Acquire), 1);
    assert_eq!(driver.captures.load(Ordering::Acquire), 1);
    assert_eq!(driver.interrupts.load(Ordering::Acquire), 0);
    assert!(!directory(&driver).exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropping_a_session_on_a_plain_thread_uses_its_original_runtime_for_cleanup() {
    let driver = driver(false);
    driver.create.add_permits(1);
    let (flags, lifetime) = lifetime();
    let session = start(driver.clone(), lifetime, Duration::from_secs(3))
        .await
        .unwrap();
    std::thread::spawn(move || drop(session)).join().unwrap();
    wait_for(|| driver.closes.load(Ordering::Acquire) == 1).await;
    assert!(!flags.released.load(Ordering::Acquire));
    driver.close.add_permits(1);
    wait_for(|| flags.released.load(Ordering::Acquire)).await;
    assert_eq!(driver.closes.load(Ordering::Acquire), 1);
    assert_eq!(driver.interrupts.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn cancelling_a_summary_during_catalog_read_finishes_that_read_but_never_creates_a_session() {
    let driver = driver(false);
    driver.catalog.forget_permits(10);
    let (flags, lifetime) = lifetime();
    let source = driver.clone();
    let caller = tokio::spawn(async move {
        crate::commit_summary::summarize_with_driver(
            source,
            "private diff",
            Some("en"),
            Some(lifetime),
        )
        .await
    });
    wait_for(|| driver.catalogs.load(Ordering::Acquire) == 1).await;
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    assert!(!flags.released.load(Ordering::Acquire));
    driver.catalog.add_permits(1);
    wait_for(|| flags.released.load(Ordering::Acquire)).await;
    assert_eq!(driver.catalogs_finished.load(Ordering::Acquire), 1);
    assert_eq!(driver.creates.load(Ordering::Acquire), 0);
    assert_eq!(driver.prompts.load(Ordering::Acquire), 0);
    assert_eq!(driver.closes.load(Ordering::Acquire), 0);
}

#[tokio::test]
async fn summary_prompt_abort_or_panic_keeps_its_original_assets_until_cleanup_finishes() {
    for panic in [false, true] {
        let driver = driver(false);
        driver.create.add_permits(1);
        driver.hold_prompt.store(!panic, Ordering::Release);
        driver.panic_prompt.store(panic, Ordering::Release);
        let (flags, lifetime) = lifetime();
        let source = driver.clone();
        let caller = tokio::spawn(async move {
            crate::commit_summary::summarize_with_driver(
                source,
                "private diff",
                Some("en"),
                Some(lifetime),
            )
            .await
        });
        wait_for(|| driver.prompts.load(Ordering::Acquire) == 1).await;
        if !panic {
            caller.abort();
        }
        assert_eq!(caller.await.unwrap_err().is_panic(), panic);
        wait_for(|| driver.closes.load(Ordering::Acquire) == 1).await;
        assert!(!flags.released.load(Ordering::Acquire));
        assert!(directory(&driver).is_dir());
        driver.close.add_permits(1);
        wait_for(|| flags.released.load(Ordering::Acquire)).await;
        assert_eq!(driver.closes.load(Ordering::Acquire), 1);
        assert_eq!(driver.interrupts.load(Ordering::Acquire), 1);
        assert!(!directory(&driver).exists());
    }
}

#[tokio::test]
async fn a_completed_summary_cannot_hide_unconfirmed_cleanup_or_release_its_assets() {
    let driver = driver(true);
    driver.create.add_permits(1);
    driver.close.add_permits(1);
    driver.close_error.store(true, Ordering::Release);
    let (flags, lifetime) = lifetime();
    let result = crate::commit_summary::summarize_with_driver(
        driver.clone(),
        "private diff",
        Some("en"),
        Some(lifetime),
    )
    .await;
    assert!(result.unwrap_err().contains("关闭尚未确认"));
    assert!(!flags.released.load(Ordering::Acquire));
    assert!(directory(&driver).is_dir());
    assert_eq!(driver.interrupts.load(Ordering::Acquire), 0);
    driver.proof.as_ref().unwrap().add_permits(1);
    wait_for(|| flags.released.load(Ordering::Acquire)).await;
    assert_eq!(driver.closes.load(Ordering::Acquire), 1);
    assert_eq!(driver.captures.load(Ordering::Acquire), 1);
}
