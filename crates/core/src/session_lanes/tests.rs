use super::*;
use agent_domain::ProviderId;
use driver_api::{
    ContextUsageSnapshot, DriverEventStream, PromptRequest, SessionHandle, SessionRequest,
};
use std::sync::atomic::{AtomicUsize, Ordering};

/// One native process per driver, like ACP: it hands out independent adapters.
struct Shared {
    executor: ExecutorRef,
    isolates: bool,
    adapters: AtomicUsize,
    context: Option<u64>,
    session_usage: bool,
}

fn executor() -> ExecutorRef {
    ExecutorRef {
        provider_id: ProviderId::parse("kimi").unwrap(),
        installation_id: Some("kimi-1".into()),
    }
}

fn shared(isolates: bool) -> Arc<Shared> {
    Arc::new(Shared {
        executor: executor(),
        isolates,
        adapters: AtomicUsize::new(0),
        context: None,
        session_usage: false,
    })
}

#[async_trait]
impl AgentDriver for Shared {
    fn executor(&self) -> &ExecutorRef {
        &self.executor
    }
    fn driver_version(&self) -> &'static str {
        "test"
    }
    fn independent_session_driver(&self) -> Result<Option<Arc<dyn AgentDriver>>, DriverError> {
        if !self.isolates {
            return Ok(None);
        }
        self.adapters.fetch_add(1, Ordering::SeqCst);
        Ok(Some(Arc::new(Shared {
            executor: self.executor.clone(),
            isolates: true,
            adapters: AtomicUsize::new(0),
            context: Some(7),
            session_usage: true,
        })))
    }
    fn session_usage_provider(self: Arc<Self>) -> Option<Arc<dyn QuotaProvider>> {
        Some(self)
    }
    async fn create_session(&self, _: SessionRequest) -> Result<SessionHandle, DriverError> {
        unreachable!()
    }
    async fn prompt(
        &self,
        _: &SessionHandle,
        _: PromptRequest,
    ) -> Result<DriverEventStream, DriverError> {
        unreachable!()
    }
    async fn interrupt(&self, _: &SessionHandle) -> Result<(), DriverError> {
        Ok(())
    }
    async fn shutdown(&self, _: SessionHandle) -> Result<(), DriverError> {
        Ok(())
    }
}

#[async_trait]
impl QuotaProvider for Shared {
    fn executor(&self) -> &ExecutorRef {
        &self.executor
    }
    async fn read_quota(&self, id: Option<&str>) -> Result<QuotaSnapshot, DriverError> {
        Ok(QuotaSnapshot {
            plan_type: Some(
                if self.session_usage {
                    "lane"
                } else {
                    "account"
                }
                .into(),
            ),
            context: self
                .context
                .filter(|_| id.is_some())
                .map(|used| ContextUsageSnapshot {
                    used_tokens: used,
                    ..Default::default()
                }),
            ..Default::default()
        })
    }
    async fn read_session_usage(&self, _: &str) -> Option<SessionUsageSnapshot> {
        self.session_usage.then(SessionUsageSnapshot::default)
    }
}

thread_local! {
    static TASKS: std::cell::RefCell<BTreeMap<String, TaskId>> = std::cell::RefCell::default();
}

fn task(name: &str) -> TaskId {
    TASKS.with(|tasks| {
        tasks
            .borrow_mut()
            .entry(name.into())
            .or_insert_with(TaskId::new)
            .clone()
    })
}

fn base_of(driver: &Arc<Shared>) -> Arc<dyn AgentDriver> {
    driver.clone()
}

#[test]
fn first_conversation_keeps_the_registry_driver_and_others_get_their_own() {
    let lanes = SessionLanes::default();
    let driver = shared(true);
    let base = base_of(&driver);
    let a = lanes.choose(&task("a"), base.clone(), None, &[]).unwrap();
    assert!(Arc::ptr_eq(&a.driver, &base));
    let b = lanes.choose(&task("b"), base.clone(), None, &[]).unwrap();
    assert!(!Arc::ptr_eq(&b.driver, &base));
    let c = lanes.choose(&task("c"), base.clone(), None, &[]).unwrap();
    assert!(!Arc::ptr_eq(&c.driver, &b.driver));
    // Later turns stay on the same process so the live native session is reused.
    let again = lanes.choose(&task("b"), base.clone(), None, &[]).unwrap();
    assert!(Arc::ptr_eq(&again.driver, &b.driver));
    assert!(Arc::ptr_eq(
        &lanes
            .choose(&task("a"), base.clone(), None, &[])
            .unwrap()
            .driver,
        &base
    ));
    assert_eq!(driver.adapters.load(Ordering::SeqCst), 2);
}

#[test]
fn multi_session_drivers_are_shared_by_every_conversation() {
    let lanes = SessionLanes::default();
    let base = base_of(&shared(false));
    for name in ["a", "b", "c"] {
        let choice = lanes.choose(&task(name), base.clone(), None, &[]).unwrap();
        assert!(Arc::ptr_eq(&choice.driver, &base));
    }
    assert!(lanes.existing(&task("b"), &executor()).is_none());
}

#[test]
fn idle_lanes_are_released_oldest_first_but_busy_ones_are_kept() {
    let lanes = SessionLanes::default();
    let base = base_of(&shared(true));
    lanes
        .choose(&task("owner"), base.clone(), None, &[])
        .unwrap();
    let first = lanes.choose(&task("l1"), base.clone(), None, &[]).unwrap();
    lanes.choose(&task("l2"), base.clone(), None, &[]).unwrap();
    lanes.choose(&task("l3"), base.clone(), None, &[]).unwrap();
    let busy = [first.driver.clone()];
    // Three idle lanes (l2, l3, l4) are kept alongside the running l1.
    lanes
        .choose(&task("l4"), base.clone(), None, &busy)
        .unwrap();
    for name in ["l1", "l2", "l3", "l4"] {
        assert!(lanes.existing(&task(name), &executor()).is_some());
    }
    // l1 is running, so l2 (the oldest idle lane) goes instead.
    lanes
        .choose(&task("l5"), base.clone(), None, &busy)
        .unwrap();
    assert!(lanes.existing(&task("l1"), &executor()).is_some());
    assert!(lanes.existing(&task("l2"), &executor()).is_none());
    for name in ["l3", "l4", "l5"] {
        assert!(lanes.existing(&task(name), &executor()).is_some());
    }
}

#[test]
fn deleting_the_owner_frees_the_registry_driver() {
    let lanes = SessionLanes::default();
    let base = base_of(&shared(true));
    lanes.choose(&task("a"), base.clone(), None, &[]).unwrap();
    lanes.choose(&task("b"), base.clone(), None, &[]).unwrap();
    lanes.forget(&task("a"));
    lanes.forget(&task("b"));
    let c = lanes.choose(&task("c"), base.clone(), None, &[]).unwrap();
    assert!(Arc::ptr_eq(&c.driver, &base));
}

#[test]
fn a_replaced_registry_driver_starts_fresh_lanes() {
    let lanes = SessionLanes::default();
    let old = base_of(&shared(true));
    lanes.choose(&task("a"), old.clone(), None, &[]).unwrap();
    let b_old = lanes.choose(&task("b"), old.clone(), None, &[]).unwrap();
    let new = base_of(&shared(true));
    let b_new = lanes.choose(&task("b"), new.clone(), None, &[]).unwrap();
    assert!(Arc::ptr_eq(&b_new.driver, &new));
    assert!(!Arc::ptr_eq(&b_new.driver, &b_old.driver));
}

#[tokio::test]
async fn lane_usage_reads_the_session_on_its_lane_and_the_account_from_the_registry() {
    let lanes = SessionLanes::default();
    let driver = shared(true);
    let base = base_of(&driver);
    let account: Arc<dyn QuotaProvider> = driver.clone();
    lanes
        .choose(&task("a"), base.clone(), Some(account.clone()), &[])
        .unwrap();
    let b = lanes
        .choose(&task("b"), base.clone(), Some(account), &[])
        .unwrap();
    let usage = b.usage.unwrap();
    assert!(usage.read_session_usage("native").await.is_some());
    let quota = usage.read_quota(Some("native")).await.unwrap();
    assert_eq!(quota.plan_type.as_deref(), Some("account"));
    assert_eq!(quota.context.unwrap().used_tokens, 7);
    let installation = usage.read_quota(None).await.unwrap();
    assert!(installation.context.is_none());
    let (_, stored) = lanes.existing(&task("b"), &executor()).unwrap();
    assert!(stored.is_some());
}
