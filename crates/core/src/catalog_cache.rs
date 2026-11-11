//! Service-owned metadata inventory. Successful scopes live until driver replacement
//! or explicit refresh; background warmup never has to finish before the UI opens.
use agent_domain::ExecutorRef;
use driver_api::{AgentDriver, DriverError, ModelCatalog, PermissionCatalog};
use futures::{FutureExt, future::BoxFuture};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::sync::{oneshot, watch};

type Outcome<T> = Result<T, DriverError>;
type Reader<T> = Arc<dyn Fn() -> BoxFuture<'static, Outcome<T>> + Send + Sync>;
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct Key {
    executor: ExecutorRef,
    generation: u64,
    model: Option<String>,
}
struct Pending<T> {
    result: watch::Receiver<Option<Outcome<T>>>,
    foreground: Arc<AtomicBool>,
}
struct Entry<T> {
    value: Option<T>,
    pending: Option<Pending<T>>,
    failure: Option<(tokio::time::Instant, DriverError)>,
}
impl<T> Default for Entry<T> {
    fn default() -> Self {
        Self {
            value: None,
            pending: None,
            failure: None,
        }
    }
}
struct Job {
    foreground: Arc<AtomicBool>,
    work: BoxFuture<'static, ()>,
}
#[derive(Default)]
struct QueueState {
    running: usize,
    jobs: VecDeque<Job>,
}
#[derive(Default)]
struct Queue {
    state: Mutex<QueueState>,
}
struct Running(Arc<Queue>);
impl Drop for Running {
    fn drop(&mut self) {
        self.0.state.lock().unwrap().running -= 1;
        self.0.pump();
    }
}
impl Queue {
    fn pump(self: &Arc<Self>) {
        let mut state = self.state.lock().unwrap();
        while state.running < 2 && !state.jobs.is_empty() {
            let index = state
                .jobs
                .iter()
                .position(|job| job.foreground.load(Ordering::Acquire))
                .unwrap_or(0);
            let job = state.jobs.remove(index).unwrap();
            state.running += 1;
            let queue = self.clone();
            tokio::spawn(async move {
                let _running = Running(queue);
                job.work.await;
            });
        }
    }
    async fn read<T: Send + 'static>(
        self: &Arc<Self>,
        foreground: Arc<AtomicBool>,
        reader: Reader<T>,
    ) -> Outcome<T> {
        let (reply, result) = oneshot::channel();
        self.state.lock().unwrap().jobs.push_back(Job {
            foreground,
            work: Box::pin(async move {
                let result = std::panic::AssertUnwindSafe(tokio::time::timeout(
                    Duration::from_secs(30),
                    reader(),
                ))
                .catch_unwind()
                .await
                .unwrap_or_else(|_| {
                    Ok(Err(DriverError::Disconnected(
                        "metadata query worker failed".into(),
                    )))
                })
                .map_err(|_| DriverError::Disconnected("metadata query timed out".into()))
                .and_then(std::convert::identity);
                let _ = reply.send(result);
            }),
        });
        self.pump();
        result
            .await
            .map_err(|_| DriverError::Disconnected("metadata query worker ended".into()))?
    }
}
type Entries<T> = Arc<Mutex<BTreeMap<Key, Arc<Mutex<Entry<T>>>>>>;
struct Inventory<T> {
    entries: Entries<T>,
    queue: Arc<Queue>,
    revision: Arc<AtomicU64>,
}
impl<T: Clone + Send + Sync + 'static> Inventory<T> {
    fn new(queue: Arc<Queue>, revision: Arc<AtomicU64>) -> Self {
        Self {
            entries: Arc::default(),
            queue,
            revision,
        }
    }
    fn seed(&self, key: Key, value: T) {
        let mut entries = self.entries.lock().unwrap();
        entries.entry(key).or_insert_with(|| {
            Arc::new(Mutex::new(Entry {
                value: Some(value),
                ..Entry::default()
            }))
        });
    }
    fn peek(&self, key: &Key) -> Option<T> {
        let entry = self.entries.lock().unwrap().get(key).cloned()?;
        entry.lock().unwrap().value.clone()
    }
    fn invalidate(&self, executor: &ExecutorRef) {
        self.entries
            .lock()
            .unwrap()
            .retain(|key, _| &key.executor != executor);
    }
    async fn get(
        &self,
        key: Key,
        refresh: bool,
        foreground: bool,
        reader: Reader<T>,
    ) -> Outcome<T> {
        let entry = self
            .entries
            .lock()
            .unwrap()
            .entry(key.clone())
            .or_default()
            .clone();
        let mut result = {
            let mut state = entry.lock().unwrap();
            if !refresh && let Some(value) = &state.value {
                return Ok(value.clone());
            }
            if let Some(pending) = &state.pending {
                if foreground {
                    pending.foreground.store(true, Ordering::Release);
                }
                pending.result.clone()
            } else {
                if !refresh
                    && let Some((until, error)) = &state.failure
                    && tokio::time::Instant::now() < *until
                {
                    return Err(error.clone());
                }
                let (reply, result) = watch::channel(None);
                let priority = Arc::new(AtomicBool::new(foreground));
                state.pending = Some(Pending {
                    result: result.clone(),
                    foreground: priority.clone(),
                });
                let queue = self.queue.clone();
                let target = entry.clone();
                let revision = self.revision.clone();
                let entries = self.entries.clone();
                tokio::spawn(async move {
                    let mut outcome = queue.read(priority.clone(), reader.clone()).await;
                    for delay in [1500, 4000] {
                        if outcome.is_ok() {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                        outcome = queue.read(priority.clone(), reader.clone()).await;
                    }
                    if refresh && outcome.is_ok() {
                        entries.lock().unwrap().retain(|candidate, _| {
                            candidate == &key
                                || candidate.executor != key.executor
                                || candidate.generation != key.generation
                        });
                    }
                    let mut state = target.lock().unwrap();
                    state.pending = None;
                    match &outcome {
                        Ok(value) => {
                            state.value = Some(value.clone());
                            state.failure = None;
                            revision.fetch_add(1, Ordering::AcqRel);
                        }
                        Err(error) => {
                            state.failure = Some((
                                tokio::time::Instant::now() + Duration::from_secs(30),
                                error.clone(),
                            ));
                        }
                    }
                    reply.send_replace(Some(outcome));
                });
                result
            }
        };
        loop {
            if let Some(outcome) = result.borrow_and_update().clone() {
                return outcome;
            }
            result
                .changed()
                .await
                .map_err(|_| DriverError::Disconnected("metadata inventory stopped".into()))?;
        }
    }
}

#[derive(Default)]
struct ModelInventory {
    catalog: ModelCatalog,
    complete: BTreeSet<String>,
}
pub(super) struct Catalogs {
    snapshots: Mutex<BTreeMap<(ExecutorRef, u64), ModelInventory>>,
    revision: Arc<AtomicU64>,
    generations: Mutex<BTreeMap<ExecutorRef, u64>>,
    models: Inventory<ModelCatalog>,
    permissions: Inventory<PermissionCatalog>,
}
impl Default for Catalogs {
    fn default() -> Self {
        let queue = Arc::new(Queue::default());
        let revision = Arc::new(AtomicU64::new(0));
        Self {
            revision: revision.clone(),
            generations: Mutex::default(),
            snapshots: Mutex::default(),
            models: Inventory::new(queue.clone(), revision.clone()),
            permissions: Inventory::new(queue, revision),
        }
    }
}
impl Catalogs {
    pub fn revision(&self) -> u64 {
        self.revision.load(Ordering::Acquire)
    }
    /// Asks clients to read catalogs again without discarding any cached inventory.
    pub fn touch(&self) {
        self.revision.fetch_add(1, Ordering::AcqRel);
    }
    pub fn replace(&self, executor: &ExecutorRef) {
        let generation = self.revision.fetch_add(1, Ordering::AcqRel) + 1;
        self.generations
            .lock()
            .unwrap()
            .insert(executor.clone(), generation);
        self.snapshots
            .lock()
            .unwrap()
            .retain(|(owner, _), _| owner != executor);
        self.models.invalidate(executor);
        self.permissions.invalidate(executor);
    }
    pub fn generation(&self, executor: &ExecutorRef) -> u64 {
        self.generations
            .lock()
            .unwrap()
            .get(executor)
            .copied()
            .unwrap_or(0)
    }
    fn merge_models(&self, key: &Key, mut catalog: ModelCatalog) -> ModelCatalog {
        let mut snapshots = self.snapshots.lock().unwrap();
        let inventory = snapshots
            .entry((key.executor.clone(), key.generation))
            .or_default();
        let selected = key.model.as_ref().or(catalog.default_model.as_ref());
        for model in &mut catalog.models {
            if selected == Some(&model.id) || !model.thinking_options.is_empty() {
                inventory.complete.insert(model.id.clone());
            } else if let Some(previous) = inventory
                .catalog
                .models
                .iter()
                .find(|old| old.id == model.id)
            {
                model
                    .thinking_options
                    .clone_from(&previous.thinking_options);
                model
                    .default_thinking
                    .clone_from(&previous.default_thinking);
            }
        }
        inventory
            .complete
            .retain(|id| catalog.models.iter().any(|model| &model.id == id));
        inventory.catalog = catalog.clone();
        catalog
    }
    /// A catalog that already carries every model's efforts needs no per-model probe.
    fn merge_complete(&self, executor: &ExecutorRef, generation: u64, mut catalog: ModelCatalog) {
        let mut snapshots = self.snapshots.lock().unwrap();
        let inventory = snapshots.entry((executor.clone(), generation)).or_default();
        // The probe selected each model in turn; the native default stays the base one.
        if inventory.catalog.default_model.is_some() {
            catalog
                .default_model
                .clone_from(&inventory.catalog.default_model);
        }
        inventory.complete = catalog
            .models
            .iter()
            .map(|model| model.id.clone())
            .collect();
        inventory.catalog = catalog;
        drop(snapshots);
        self.revision.fetch_add(1, Ordering::AcqRel);
    }
    fn model_reader(
        driver: Arc<dyn AgentDriver>,
        model: Option<String>,
        refresh: bool,
    ) -> Reader<ModelCatalog> {
        Arc::new(move || {
            let (driver, model) = (driver.clone(), model.clone());
            Box::pin(async move {
                if refresh {
                    driver
                        .refresh_model_catalog_in_workspace(model.as_deref(), None)
                        .await
                } else {
                    driver
                        .model_catalog_in_workspace(model.as_deref(), None)
                        .await
                }
            })
        })
    }
    pub async fn models(
        &self,
        driver: Arc<dyn AgentDriver>,
        generation: u64,
        model: Option<String>,
        refresh: bool,
        foreground: bool,
    ) -> Outcome<ModelCatalog> {
        let key = Key {
            executor: driver.executor().clone(),
            generation,
            model: model.clone(),
        };
        if !refresh
            && model.is_some()
            && !self
                .snapshots
                .lock()
                .unwrap()
                .contains_key(&(key.executor.clone(), generation))
        {
            let mut base = key.clone();
            base.model = None;
            let catalog = self
                .models
                .get(
                    base.clone(),
                    false,
                    foreground,
                    Self::model_reader(driver.clone(), None, false),
                )
                .await?;
            if self.generation(&key.executor) != generation {
                return Ok(catalog);
            }
            self.merge_models(&base, catalog);
        }
        if !refresh
            && let Some(inventory) = self
                .snapshots
                .lock()
                .unwrap()
                .get(&(key.executor.clone(), generation))
            && model.as_ref().is_none_or(|id| {
                inventory.complete.contains(id)
                    || !inventory
                        .catalog
                        .models
                        .iter()
                        .any(|option| &option.id == id)
            })
        {
            return Ok(inventory.catalog.clone());
        }
        let catalog = self
            .models
            .get(
                key.clone(),
                refresh,
                foreground,
                Self::model_reader(driver, model, refresh),
            )
            .await?;
        if self.generation(&key.executor) != generation {
            return Ok(catalog);
        }
        Ok(self.merge_models(&key, catalog))
    }
    pub async fn permissions(
        &self,
        driver: Arc<dyn AgentDriver>,
        generation: u64,
        refresh: bool,
        foreground: bool,
    ) -> Outcome<PermissionCatalog> {
        let key = Key {
            executor: driver.executor().clone(),
            generation,
            model: None,
        };
        self.permissions
            .get(
                key,
                refresh,
                foreground,
                Arc::new(move || {
                    let driver = driver.clone();
                    Box::pin(async move { driver.permission_catalog(None).await })
                }),
            )
            .await
    }
    /// The permission catalog already read for this installation, without
    /// starting a read.
    pub fn cached_permissions(&self, executor: &ExecutorRef) -> Option<PermissionCatalog> {
        self.permissions.peek(&Key {
            executor: executor.clone(),
            generation: self.generation(executor),
            model: None,
        })
    }
    pub fn seed_startup(&self, driver: &Arc<dyn AgentDriver>) {
        let generation = self.generation(driver.executor());
        let key = Key {
            executor: driver.executor().clone(),
            generation,
            model: None,
        };
        let startup = driver.take_startup_catalogs();
        if let Some(value) = startup.models {
            if let Some(default) = &value.default_model {
                let mut selected = key.clone();
                selected.model = Some(default.clone());
                self.models.seed(selected, value.clone());
            }
            self.merge_models(&key, value.clone());
            self.models.seed(key.clone(), value);
        }
        if let Some(value) = startup.permissions {
            self.permissions.seed(key, value);
        }
    }
    pub fn warm(self: &Arc<Self>, driver: &Arc<dyn AgentDriver>) {
        self.seed_startup(driver);
        let generation = self.generation(driver.executor());
        let (cache, driver) = (Arc::downgrade(self), Arc::downgrade(driver));
        tokio::spawn(async move {
            loop {
                let Some((cache, driver)) = cache.upgrade().zip(driver.upgrade()) else {
                    return;
                };
                if cache.generation(driver.executor()) != generation {
                    return;
                }
                let models = cache
                    .models(driver.clone(), generation, None, false, false)
                    .await;
                if cache.generation(driver.executor()) != generation {
                    return;
                }
                let permissions = cache
                    .permissions(driver.clone(), generation, false, false)
                    .await;
                let mut complete = models.is_ok() && permissions.is_ok();
                let all = match &models {
                    Ok(_) => driver.model_catalog_with_all_efforts().await,
                    Err(_) => None,
                };
                if cache.generation(driver.executor()) != generation {
                    return;
                }
                let filled = match all {
                    Some(Ok(catalog)) => {
                        cache.merge_complete(driver.executor(), generation, catalog);
                        true
                    }
                    Some(Err(_)) => {
                        complete = false;
                        true
                    }
                    None => false,
                };
                if !filled && let Ok(models) = models {
                    for model in models.models {
                        if cache.generation(driver.executor()) != generation {
                            return;
                        }
                        // Most protocols return every model's efforts up front.
                        // Selected-only protocols are filled quietly into the same inventory.
                        complete &= cache
                            .models(driver.clone(), generation, Some(model.id), false, false)
                            .await
                            .is_ok();
                    }
                }
                if complete {
                    return;
                }
                drop(cache);
                tokio::time::sleep(Duration::from_secs(30)).await;
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    struct ProbeDriver {
        base: driver_api::FakeDriver,
        calls: AtomicUsize,
        fail: AtomicBool,
        startup: Mutex<driver_api::models::StartupCatalogs>,
        /// Reports every model's efforts in one probe, as the `OpenCode` driver does.
        all_efforts: AtomicUsize,
    }
    #[async_trait::async_trait]
    impl AgentDriver for ProbeDriver {
        fn executor(&self) -> &ExecutorRef {
            self.base.executor()
        }
        fn driver_version(&self) -> &'static str {
            "test"
        }
        fn take_startup_catalogs(&self) -> driver_api::models::StartupCatalogs {
            std::mem::take(&mut *self.startup.lock().unwrap())
        }
        async fn model_catalog_in_workspace(
            &self,
            model: Option<&str>,
            workspace: Option<&str>,
        ) -> Outcome<ModelCatalog> {
            assert!(
                workspace.is_none(),
                "menu inventory must not read a project"
            );
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.fail.load(Ordering::SeqCst) {
                Err(DriverError::Disconnected("offline".into()))
            } else {
                Ok(fixture_catalog(model))
            }
        }
        async fn model_catalog_with_all_efforts(&self) -> Option<Outcome<ModelCatalog>> {
            if self.all_efforts.load(Ordering::SeqCst) == 0 {
                return None;
            }
            self.all_efforts.fetch_add(1, Ordering::SeqCst);
            let mut catalog = fixture_catalog(None);
            for model in &mut catalog.models {
                model.thinking_options = vec![driver_api::ThinkingOption {
                    id: "high".into(),
                    label: "High".into(),
                }];
            }
            Some(Ok(catalog))
        }
        async fn permission_catalog(&self, _: Option<&str>) -> Outcome<PermissionCatalog> {
            panic!("seeded permission inventory must be reused")
        }
        async fn create_session(
            &self,
            request: driver_api::SessionRequest,
        ) -> Outcome<driver_api::SessionHandle> {
            self.base.create_session(request).await
        }
        async fn prompt(
            &self,
            handle: &driver_api::SessionHandle,
            request: driver_api::PromptRequest,
        ) -> Outcome<driver_api::DriverEventStream> {
            self.base.prompt(handle, request).await
        }
        async fn interrupt(&self, handle: &driver_api::SessionHandle) -> Outcome<()> {
            self.base.interrupt(handle).await
        }
        async fn shutdown(&self, handle: driver_api::SessionHandle) -> Outcome<()> {
            self.base.shutdown(handle).await
        }
    }
    fn fixture_catalog(selected: Option<&str>) -> ModelCatalog {
        ModelCatalog {
            default_model: Some("fixture".into()),
            models: ["fixture", "other"]
                .into_iter()
                .map(|id| {
                    let mut model = driver_api::ModelOption::new(id, id);
                    if selected.unwrap_or("fixture") == id {
                        model.thinking_options = vec![driver_api::ThinkingOption {
                            id: "high".into(),
                            label: "High".into(),
                        }];
                    }
                    model
                })
                .collect(),
        }
    }
    fn probe(seeded_models: bool, fail: bool) -> Arc<ProbeDriver> {
        Arc::new(ProbeDriver {
            base: driver_api::FakeDriver::new("catalog-test").unwrap(),
            calls: AtomicUsize::new(0),
            fail: AtomicBool::new(fail),
            all_efforts: AtomicUsize::new(0),
            startup: Mutex::new(driver_api::models::StartupCatalogs {
                models: seeded_models.then(|| {
                    let mut catalog = fixture_catalog(None);
                    catalog.models.retain(|model| model.id == "fixture");
                    catalog
                }),
                permissions: Some(PermissionCatalog::default()),
            }),
        })
    }

    #[tokio::test]
    async fn one_probe_with_every_models_efforts_replaces_the_per_model_fill() {
        let cache = Arc::new(Catalogs::default());
        let probe = probe(true, false);
        probe.all_efforts.store(1, Ordering::SeqCst);
        let driver: Arc<dyn AgentDriver> = probe.clone();
        cache.replace(driver.executor());
        cache.seed_startup(&driver);
        let generation = cache.generation(driver.executor());
        let before = cache.revision();
        cache.warm(&driver);
        while probe.all_efforts.load(Ordering::SeqCst) < 2 || cache.revision() == before {
            tokio::task::yield_now().await;
        }
        let other = cache
            .models(
                driver.clone(),
                generation,
                Some("other".into()),
                false,
                true,
            )
            .await
            .unwrap();
        assert!(
            other
                .models
                .iter()
                .all(|model| !model.thinking_options.is_empty())
        );
        assert_eq!(other.default_model.as_deref(), Some("fixture"));
        // Neither warmup nor the selection started a probe of its own per model.
        assert_eq!(probe.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn startup_results_are_shared_by_all_consumers_of_the_agent() {
        let cache = Arc::new(Catalogs::default());
        let probe = probe(true, false);
        let driver: Arc<dyn AgentDriver> = probe.clone();
        cache.replace(driver.executor());
        cache.seed_startup(&driver);
        cache.warm(&driver);
        let generation = cache.generation(driver.executor());
        for _ in 0..4 {
            cache
                .models(driver.clone(), generation, None, false, true)
                .await
                .unwrap();
            cache
                .permissions(driver.clone(), generation, false, true)
                .await
                .unwrap();
        }
        assert_eq!(probe.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn selected_model_joins_warmup_and_merges_efforts_into_the_agent_inventory() {
        let cache = Arc::new(Catalogs::default());
        let probe = probe(false, false);
        let driver: Arc<dyn AgentDriver> = probe.clone();
        cache.replace(driver.executor());
        cache.seed_startup(&driver);
        let generation = cache.generation(driver.executor());
        let (warm, selected) = tokio::join!(
            cache.models(driver.clone(), generation, None, false, false),
            cache.models(
                driver.clone(),
                generation,
                Some("fixture".into()),
                false,
                true
            ),
        );
        assert_eq!(warm.unwrap(), selected.unwrap());
        assert_eq!(probe.calls.load(Ordering::SeqCst), 1);
        cache
            .models(
                driver.clone(),
                generation,
                Some("other".into()),
                false,
                true,
            )
            .await
            .unwrap();
        let snapshot = cache
            .models(driver.clone(), generation, None, false, true)
            .await
            .unwrap();
        assert!(
            snapshot
                .models
                .iter()
                .all(|model| model.thinking_options.len() == 1)
        );
        for id in ["fixture", "other", "missing"] {
            cache
                .models(driver.clone(), generation, Some(id.into()), false, true)
                .await
                .unwrap();
        }
        assert_eq!(probe.calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn failed_startup_warmup_recovers_in_the_background_without_any_menu_request() {
        let cache = Arc::new(Catalogs::default());
        let probe = probe(false, true);
        let driver: Arc<dyn AgentDriver> = probe.clone();
        cache.replace(driver.executor());
        cache.warm(&driver);
        tokio::time::sleep(Duration::from_secs(7)).await;
        assert_eq!(probe.calls.load(Ordering::SeqCst), 3);
        probe.fail.store(false, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_secs(31)).await;
        assert_eq!(probe.calls.load(Ordering::SeqCst), 5);
        cache
            .models(
                driver.clone(),
                cache.generation(driver.executor()),
                None,
                false,
                true,
            )
            .await
            .unwrap();
        assert_eq!(probe.calls.load(Ordering::SeqCst), 5);
    }

    fn key(model: &str) -> Key {
        Key {
            executor: ExecutorRef {
                provider_id: agent_domain::ProviderId::parse("catalog-test").unwrap(),
                installation_id: Some("installation".into()),
            },
            generation: 1,
            model: Some(model.into()),
        }
    }
    fn inventory<T: Clone + Send + Sync + 'static>() -> Arc<Inventory<T>> {
        Arc::new(Inventory::new(Arc::default(), Arc::new(AtomicU64::new(0))))
    }
    fn counted(calls: &Arc<AtomicUsize>, value: u32) -> Reader<u32> {
        let calls = calls.clone();
        Arc::new(move || {
            calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move { Ok(value) })
        })
    }
    #[tokio::test(start_paused = true)]
    async fn successful_inventory_does_not_expire_and_refresh_keeps_last_success_on_failure() {
        let cache = inventory();
        let calls = Arc::new(AtomicUsize::new(0));
        assert_eq!(
            cache
                .get(key("a"), false, true, counted(&calls, 1))
                .await
                .unwrap(),
            1
        );
        tokio::time::advance(Duration::from_hours(24)).await;
        assert_eq!(
            cache
                .get(key("a"), false, true, counted(&calls, 2))
                .await
                .unwrap(),
            1
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let errors = Arc::new(AtomicUsize::new(0));
        let count = errors.clone();
        let failed: Reader<u32> = Arc::new(move || {
            count.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Err(DriverError::Disconnected("offline".into())) })
        });
        assert!(
            cache
                .get(key("a"), true, true, failed.clone())
                .await
                .is_err()
        );
        assert_eq!(errors.load(Ordering::SeqCst), 3);
        assert_eq!(
            cache
                .get(key("a"), false, true, counted(&calls, 3))
                .await
                .unwrap(),
            1
        );
        assert!(
            cache
                .get(key("b"), false, true, failed.clone())
                .await
                .is_err()
        );
        let before = errors.load(Ordering::SeqCst);
        assert!(cache.get(key("b"), false, true, failed).await.is_err());
        assert_eq!(errors.load(Ordering::SeqCst), before);
        tokio::time::advance(Duration::from_secs(31)).await;
        assert_eq!(
            cache
                .get(key("b"), false, true, counted(&calls, 4))
                .await
                .unwrap(),
            4
        );
    }
    #[tokio::test]
    async fn foreground_missing_scope_is_promoted_ahead_of_queued_warmup_and_shares_the_read() {
        let cache = inventory();
        let started = Arc::new(Mutex::new(Vec::new()));
        let releases: Vec<_> = (0..4)
            .map(|_| Arc::new(tokio::sync::Notify::new()))
            .collect();
        let reader = |index: usize| -> Reader<u32> {
            let (started, release) = (started.clone(), releases[index].clone());
            Arc::new(move || {
                let (started, release) = (started.clone(), release.clone());
                Box::pin(async move {
                    started.lock().unwrap().push(index);
                    release.notified().await;
                    Ok(u32::try_from(index).unwrap())
                })
            })
        };
        let mut requests = vec![];
        for index in 0..4 {
            let (cache, read) = (cache.clone(), reader(index));
            requests.push(tokio::spawn(async move {
                cache.get(key(&index.to_string()), false, false, read).await
            }));
            tokio::task::yield_now().await;
        }
        while cache.queue.state.lock().unwrap().jobs.len() < 2 {
            tokio::task::yield_now().await;
        }
        assert_eq!(started.lock().unwrap().len(), 2);
        let (selected, read) = (cache.clone(), reader(3));
        let selected = tokio::spawn(async move { selected.get(key("3"), false, true, read).await });
        tokio::task::yield_now().await;
        releases[0].notify_one();
        while !started.lock().unwrap().contains(&3) {
            tokio::task::yield_now().await;
        }
        assert!(!started.lock().unwrap().contains(&2));
        for release in &releases[1..] {
            release.notify_one();
        }
        for request in requests {
            request.await.unwrap().unwrap();
        }
        selected.await.unwrap().unwrap();
        assert_eq!(started.lock().unwrap().len(), 4);
    }
    #[tokio::test]
    async fn refreshing_one_model_invalidates_siblings_without_crossing_agents() {
        let cache = inventory();
        let calls = Arc::new(AtomicUsize::new(0));
        let mut project = key("b");
        project.executor.installation_id = Some("other-agent".into());
        cache
            .get(key("a"), false, true, counted(&calls, 1))
            .await
            .unwrap();
        cache
            .get(key("b"), false, true, counted(&calls, 2))
            .await
            .unwrap();
        cache
            .get(project.clone(), false, true, counted(&calls, 3))
            .await
            .unwrap();
        cache
            .get(key("a"), true, true, counted(&calls, 4))
            .await
            .unwrap();
        assert_eq!(
            cache
                .get(key("b"), false, true, counted(&calls, 5))
                .await
                .unwrap(),
            5
        );
        assert_eq!(
            cache
                .get(project, false, true, counted(&calls, 6))
                .await
                .unwrap(),
            3
        );
    }
    #[tokio::test]
    async fn a_retired_driver_query_cannot_overwrite_a_replacement_inventory() {
        let cache = inventory();
        let release = Arc::new(tokio::sync::Notify::new());
        let pending: Reader<u32> = {
            let release = release.clone();
            Arc::new(move || {
                let release = release.clone();
                Box::pin(async move {
                    release.notified().await;
                    Ok(1)
                })
            })
        };
        let old = {
            let cache = cache.clone();
            tokio::spawn(async move { cache.get(key("a"), false, true, pending).await })
        };
        tokio::task::yield_now().await;
        cache.invalidate(&key("a").executor);
        let calls = Arc::new(AtomicUsize::new(0));
        assert_eq!(
            cache
                .get(key("a"), false, true, counted(&calls, 2))
                .await
                .unwrap(),
            2
        );
        release.notify_one();
        old.await.unwrap().unwrap();
        assert_eq!(
            cache
                .get(key("a"), false, true, counted(&calls, 3))
                .await
                .unwrap(),
            2
        );
    }
}
