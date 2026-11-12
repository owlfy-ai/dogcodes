//! Turn queue for agents that declare `runs_one_turn_at_a_time`: one turn at a
//! time across every conversation. No shipped driver needs it today.
//! A later turn is accepted at once, tells its conversation that it is waiting,
//! and starts by itself when the running turn ends. Stopping it while it waits
//! cancels it without touching the agent.
use agent_domain::user_text;
use crate::{AgentDriver, Core, EventSource, ExecutorRef, TaskId, run_control::Control};
use agent_protocol::ActiveRunSummary;
use serde_json::json;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use tokio::sync::OwnedMutexGuard;

pub(crate) const WAITING_EVENT: &str = "turn.waiting_for_agent";

#[derive(Default)]
pub(crate) struct ExclusiveTurns {
    gates: Mutex<BTreeMap<ExecutorRef, Arc<tokio::sync::Mutex<()>>>>,
}

/// Held for the whole turn; the next queued turn starts when it is dropped.
pub(crate) struct TurnSlot(#[allow(dead_code)] Option<OwnedMutexGuard<()>>);

impl ExclusiveTurns {
    fn gate(&self, executor: &ExecutorRef) -> Arc<tokio::sync::Mutex<()>> {
        self.gates
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(executor.clone())
            .or_default()
            .clone()
    }
}

fn agent_name(provider: &str) -> &str {
    match provider {
        "antigravity" => "Antigravity",
        other => other,
    }
}

/// Stored as an i18n code; the desktop translates `backendText.agentRunsOneTaskAtATime`.
pub(crate) fn waiting_message(provider: &str) -> String {
    user_text("backendText.agentRunsOneTaskAtATime", &[agent_name(provider)])
}

impl Core {
    /// `None` when the run was stopped while it waited.
    pub(crate) async fn take_turn_slot(
        &self,
        task: &TaskId,
        active: &ActiveRunSummary,
        driver: &Arc<dyn AgentDriver>,
        control: &Control,
    ) -> Option<TurnSlot> {
        if !driver.runs_one_turn_at_a_time() {
            return Some(TurnSlot(None));
        }
        let gate = self.exclusive_turns.gate(&active.executor);
        if let Ok(slot) = gate.clone().try_lock_owned() {
            return Some(TurnSlot(Some(slot)));
        }
        let provider = active.executor.provider_id.as_str();
        let _ = self
            .append_runtime_event(
                task,
                WAITING_EVENT,
                json!({"taskId":task,"turnId":active.turn_id,"runId":active.run_id,
                    "providerId":provider,"message":waiting_message(provider)}),
                EventSource::Hub,
            )
            .await;
        tokio::select! {
            slot = gate.lock_owned() => Some(TurnSlot(Some(slot))),
            () = control.stop_requested() => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_waiting_notice_names_the_agent() {
        assert_eq!(
            waiting_message("antigravity"),
            "backendText.agentRunsOneTaskAtATime\u{1e}[\"Antigravity\"]"
        );
    }

    #[tokio::test]
    async fn one_gate_per_agent() {
        let turns = ExclusiveTurns::default();
        let agent = ExecutorRef {
            provider_id: agent_domain::ProviderId::parse("antigravity").unwrap(),
            installation_id: Some("a".into()),
        };
        let other = ExecutorRef {
            installation_id: Some("b".into()),
            ..agent.clone()
        };
        let held = turns.gate(&agent).try_lock_owned().unwrap();
        assert!(turns.gate(&agent).try_lock_owned().is_err());
        assert!(turns.gate(&other).try_lock_owned().is_ok());
        drop(held);
        assert!(turns.gate(&agent).try_lock_owned().is_ok());
    }
}
