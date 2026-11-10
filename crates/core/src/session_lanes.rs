//! Per-conversation driver lanes for agents whose registry driver shares one
//! native process (ACP, `ZCode`, Antigravity). Such a process runs one turn at a
//! time, so concurrent conversations must not share it. The first conversation
//! keeps the warm registry driver; every other conversation gets its own adapter
//! from `conversation_lane_driver`, kept for later turns so its live native
//! session is reused. Drivers that already isolate sessions, or whose native
//! processes must never overlap, return no adapter and keep using the registry
//! driver directly.
use crate::{AgentDriver, Core, CoreError, ExecutorRef, QuotaProvider, TaskId};
use async_trait::async_trait;
use driver_api::{DriverError, QuotaSnapshot, usage::SessionUsageSnapshot};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, Weak},
};

/// Idle private lanes kept per agent; older idle ones are released and their
/// conversations restore their native session on a fresh lane next turn.
const IDLE_LANES_PER_AGENT: usize = 3;

struct Lane {
    driver: Arc<dyn AgentDriver>,
    usage: Option<Arc<dyn QuotaProvider>>,
    used: u64,
}

struct AgentLanes {
    base: Weak<dyn AgentDriver>,
    /// The conversation that runs on the registry driver itself.
    base_owner: Option<TaskId>,
    lanes: BTreeMap<TaskId, Lane>,
}

impl AgentLanes {
    fn new(base: &Arc<dyn AgentDriver>) -> Self {
        Self {
            base: Arc::downgrade(base),
            base_owner: None,
            lanes: BTreeMap::new(),
        }
    }
    fn serves(&self, base: &Arc<dyn AgentDriver>) -> bool {
        std::ptr::addr_eq(self.base.as_ptr(), Arc::as_ptr(base))
    }
}

#[derive(Default)]
pub(crate) struct SessionLanes {
    agents: Mutex<BTreeMap<ExecutorRef, AgentLanes>>,
    clock: std::sync::atomic::AtomicU64,
}

/// What a turn runs on: the driver and the usage provider frozen with it.
type ExistingLane = (Arc<dyn AgentDriver>, Option<Arc<dyn QuotaProvider>>);

pub(crate) struct LaneChoice {
    pub driver: Arc<dyn AgentDriver>,
    pub usage: Option<Arc<dyn QuotaProvider>>,
}

impl SessionLanes {
    fn tick(&self) -> u64 {
        self.clock
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            .saturating_add(1)
    }

    /// `busy` holds the drivers that currently own a run; they are never released.
    pub(crate) fn choose(
        &self,
        task: &TaskId,
        base: Arc<dyn AgentDriver>,
        account: Option<Arc<dyn QuotaProvider>>,
        busy: &[Arc<dyn AgentDriver>],
    ) -> Result<LaneChoice, DriverError> {
        let shared = LaneChoice {
            driver: Arc::clone(&base),
            usage: account.clone(),
        };
        let executor = base.executor().clone();
        let used = self.tick();
        let mut agents = self
            .agents
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let agent = agents
            .entry(executor)
            .or_insert_with(|| AgentLanes::new(&base));
        if !agent.serves(&base) {
            // The registry replaced this agent; lanes of the old one stay with
            // their running turns only.
            *agent = AgentLanes::new(&base);
        }
        if let Some(lane) = agent.lanes.get_mut(task) {
            lane.used = used;
            return Ok(LaneChoice {
                driver: Arc::clone(&lane.driver),
                usage: lane.usage.clone(),
            });
        }
        if agent.base_owner.as_ref().is_none_or(|owner| owner == task) {
            agent.base_owner = Some(task.clone());
            return Ok(shared);
        }
        let Some(driver) = base.conversation_lane_driver()? else {
            // Multi-session driver: every conversation may share it.
            return Ok(shared);
        };
        let usage = Arc::clone(&driver)
            .session_usage_provider()
            .map(|lane| -> Arc<dyn QuotaProvider> {
                Arc::new(LaneUsage {
                    executor: base.executor().clone(),
                    account: account.clone(),
                    lane,
                })
            })
            .or(account);
        release_idle(agent, busy);
        agent.lanes.insert(
            task.clone(),
            Lane {
                driver: Arc::clone(&driver),
                usage: usage.clone(),
                used,
            },
        );
        Ok(LaneChoice { driver, usage })
    }

    /// The private lane a conversation already has, if any.
    pub(crate) fn existing(&self, task: &TaskId, executor: &ExecutorRef) -> Option<ExistingLane> {
        let agents = self
            .agents
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let agent = agents.get(executor)?;
        agent.base.upgrade()?;
        agent
            .lanes
            .get(task)
            .map(|lane| (Arc::clone(&lane.driver), lane.usage.clone()))
    }

    /// A deleted conversation releases its lane and its claim on the registry driver.
    pub(crate) fn forget(&self, task: &TaskId) {
        let mut agents = self
            .agents
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for agent in agents.values_mut() {
            agent.lanes.remove(task);
            if agent.base_owner.as_ref() == Some(task) {
                agent.base_owner = None;
            }
        }
    }
}

fn release_idle(agent: &mut AgentLanes, busy: &[Arc<dyn AgentDriver>]) {
    let mut idle: Vec<_> = agent
        .lanes
        .iter()
        .filter(|(_, lane)| !busy.iter().any(|d| Arc::ptr_eq(d, &lane.driver)))
        .map(|(task, lane)| (lane.used, task.clone()))
        .collect();
    if idle.len() < IDLE_LANES_PER_AGENT {
        return;
    }
    idle.sort();
    let excess = idle.len() + 1 - IDLE_LANES_PER_AGENT;
    for (_, task) in idle.into_iter().take(excess) {
        agent.lanes.remove(&task);
    }
}

/// Account quota from the registry provider, session context and usage from the lane.
struct LaneUsage {
    executor: ExecutorRef,
    account: Option<Arc<dyn QuotaProvider>>,
    lane: Arc<dyn QuotaProvider>,
}

#[async_trait]
impl QuotaProvider for LaneUsage {
    fn executor(&self) -> &ExecutorRef {
        &self.executor
    }
    async fn read_quota(&self, id: Option<&str>) -> Result<QuotaSnapshot, DriverError> {
        self.read_quota_with_refresh(id, false).await
    }
    async fn read_quota_with_refresh(
        &self,
        id: Option<&str>,
        force: bool,
    ) -> Result<QuotaSnapshot, DriverError> {
        let Some(account) = &self.account else {
            return self.lane.read_quota_with_refresh(id, force).await;
        };
        if id.is_none() {
            return account.read_quota_with_refresh(None, force).await;
        }
        let (account, lane) = futures::join!(
            account.read_quota_with_refresh(None, force),
            self.lane.read_quota(id)
        );
        match (account, lane) {
            (Ok(mut quota), Ok(lane)) => {
                quota.context = lane.context.or(quota.context);
                Ok(quota)
            }
            (Ok(quota), Err(_)) => Ok(quota),
            (Err(_), Ok(lane)) => Ok(lane),
            (Err(error), Err(_)) => Err(error),
        }
    }
    async fn read_session_usage(&self, id: &str) -> Option<SessionUsageSnapshot> {
        self.lane.read_session_usage(id).await
    }
}

impl Core {
    fn busy_drivers(&self) -> Vec<Arc<dyn AgentDriver>> {
        self.owned_runs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .cloned()
            .collect()
    }

    /// The driver a conversation's next turn runs on.
    pub(crate) fn task_lane(
        &self,
        task: &TaskId,
        base: Arc<dyn AgentDriver>,
        account: Option<Arc<dyn QuotaProvider>>,
    ) -> Result<LaneChoice, CoreError> {
        let busy = self.busy_drivers();
        self.session_lanes
            .choose(task, base, account, &busy)
            .map_err(Into::into)
    }
}

#[cfg(test)]
mod tests;
