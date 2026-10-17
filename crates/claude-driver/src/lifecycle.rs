//! Native root results do not settle a turn while its Agent/Workflow still runs.
use driver_api::DriverEvent;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use tokio::time::{Duration, Instant};

pub(super) enum Command {
    Begin,
    Cancel,
    Close,
}

pub(super) const CANCEL_TIMEOUT: Duration = Duration::from_secs(3);
const QUIET: Duration = Duration::from_millis(200);

struct Task {
    generation: u64,
    holds_turn: bool,
    tool_id: Option<String>,
}

#[derive(Default)]
pub(super) struct Lifecycle {
    generation: u64,
    tasks: HashMap<String, Task>,
    tool_names: HashMap<String, String>,
    tool_generations: HashMap<String, u64>,
    workflow_agents: HashMap<String, String>,
    settled: HashSet<String>,
    root_live: bool,
    terminal: Option<DriverEvent>,
    held: bool,
    quiet_since: Option<Instant>,
    cancel_since: Option<Instant>,
    stopped: HashSet<String>,
    interrupt_sent: bool,
}

pub(super) enum Action {
    Stop(String),
    Interrupt,
    Finish(DriverEvent),
    Close,
}

impl Lifecycle {
    pub fn begin(&mut self) {
        self.generation += 1;
        self.root_live = true;
        self.terminal = None;
        self.held = false;
        self.quiet_since = None;
        self.cancel_since = None;
        self.stopped.clear();
        self.interrupt_sent = false;
    }

    pub fn observe(&mut self, raw: &Value) {
        if super::autonomous::is_root_output(raw)
            || (raw["type"] == "system" && raw["subtype"] == "init")
        {
            self.root_live = true;
            self.quiet_since = None;
        }
        if raw["type"] == "assistant"
            && let Some(blocks) = raw.pointer("/message/content").and_then(Value::as_array)
        {
            for block in blocks {
                if block["type"] == "tool_use"
                    && let (Some(id), Some(name)) = (block["id"].as_str(), block["name"].as_str())
                {
                    self.tool_names.insert(id.into(), name.into());
                    self.tool_generations
                        .entry(id.into())
                        .or_insert(self.generation);
                }
            }
        }
        self.observe_tool_result(raw);
        if raw["type"] != "system" {
            return;
        }
        match raw["subtype"].as_str() {
            Some("task_started" | "task_progress") => {
                self.task_started(raw);
                if let (Some(task), Some(entries)) =
                    (raw["task_id"].as_str(), raw["workflow_progress"].as_array())
                {
                    for agent in entries {
                        if agent["type"] == "workflow_agent"
                            && let Some(id) = agent["agentId"].as_str()
                        {
                            self.workflow_agents.insert(id.into(), task.into());
                        }
                    }
                }
            }
            Some("task_notification")
                if matches!(
                    raw["status"].as_str(),
                    Some("completed" | "failed" | "stopped" | "interrupted")
                ) =>
            {
                if let Some(id) = raw["task_id"].as_str() {
                    self.tasks.remove(id);
                    self.settled.insert(id.into());
                }
            }
            Some("background_tasks_changed") => {
                if let Some(tasks) = raw["tasks"].as_array()
                    && tasks
                        .iter()
                        .all(|task| task["task_id"].as_str().is_some_and(|id| !id.is_empty()))
                {
                    // The native list is authoritative for occupancy, including tasks
                    // announced before this connection saw their individual start.
                    let ids: HashSet<_> =
                        tasks.iter().filter_map(|t| t["task_id"].as_str()).collect();
                    self.settled.extend(
                        self.tasks
                            .keys()
                            .filter(|id| !ids.contains(id.as_str()))
                            .cloned(),
                    );
                    self.tasks.retain(|id, _| ids.contains(id.as_str()));
                    for task in tasks {
                        self.task_started(task);
                    }
                }
            }
            _ => {}
        }
    }

    fn task_started(&mut self, raw: &Value) {
        let Some(id) = raw["task_id"].as_str().filter(|id| !id.is_empty()) else {
            return;
        };
        if self.settled.contains(id) {
            return;
        }
        let name = raw["tool_use_id"]
            .as_str()
            .and_then(|id| self.tool_names.get(id));
        let holds_turn = name.is_none_or(|name| name != "Bash") && raw["task_type"] != "local_bash";
        self.tasks.entry(id.into()).or_insert(Task {
            generation: raw["tool_use_id"]
                .as_str()
                .and_then(|id| self.tool_generations.get(id))
                .copied()
                .unwrap_or(self.generation),
            holds_turn,
            tool_id: raw["tool_use_id"].as_str().map(str::to_owned),
        });
        if holds_turn {
            self.held = true;
        }
        self.quiet_since = None;
    }

    fn observe_tool_result(&mut self, raw: &Value) {
        if raw["type"] != "user" {
            return;
        }
        let Some(blocks) = raw.pointer("/message/content").and_then(Value::as_array) else {
            return;
        };
        let results: Vec<_> = blocks
            .iter()
            .filter(|b| b["type"] == "tool_result")
            .collect();
        if results.len() != 1 {
            return;
        }
        let block = results[0];
        let Some(tool) = block["tool_use_id"].as_str() else {
            return;
        };
        let native = raw
            .get("tool_use_result")
            .or_else(|| raw.get("toolUseResult"))
            .unwrap_or(&Value::Null);
        let name = self.tool_names.get(tool).map(String::as_str);
        let task = match name {
            Some("Bash") => native["backgroundTaskId"].as_str(),
            Some("Agent" | "Task") if native["isAsync"] == true || native["is_async"] == true => {
                native["agentId"]
                    .as_str()
                    .or_else(|| native["agent_id"].as_str())
                    .or_else(|| native["task_id"].as_str())
            }
            Some("Workflow")
                if matches!(
                    native["status"].as_str(),
                    Some("async_launched" | "remote_launched")
                ) =>
            {
                native["taskId"].as_str()
            }
            _ => None,
        };
        if block["is_error"] != true
            && let Some(task) = task
        {
            self.task_started(&serde_json::json!({"task_id":task,"tool_use_id":tool}));
        } else if matches!(name, Some("Agent" | "Task" | "Workflow")) && native.is_object() {
            self.tasks
                .retain(|_, task| task.tool_id.as_deref() != Some(tool));
        }
    }

    pub fn finished_agent_scopes(&self, task: &str) -> Vec<String> {
        std::iter::once(task.to_owned())
            .chain(
                self.workflow_agents
                    .iter()
                    .filter(|(_, owner)| owner.as_str() == task)
                    .map(|(agent, _)| agent.clone()),
            )
            .collect()
    }

    pub fn result(&mut self, event: DriverEvent) {
        self.root_live = false;
        self.interrupt_sent = false;
        self.terminal = Some(event);
        self.quiet_since = None;
    }

    pub fn cancel(&mut self) {
        self.cancel_since.get_or_insert_with(Instant::now);
        self.quiet_since = None;
    }

    pub fn cancelling(&self) -> bool {
        self.cancel_since.is_some()
    }
    pub fn waiting(&self) -> bool {
        self.cancelling() || self.terminal.is_some()
    }
    pub fn has_agents(&self) -> bool {
        self.tasks.values().any(|t| t.holds_turn)
    }

    pub fn poll(&mut self, pending_agent_interactions: bool) -> Vec<Action> {
        let now = Instant::now();
        let mut actions = Vec::new();
        if let Some(start) = self.cancel_since {
            if now.duration_since(start) >= CANCEL_TIMEOUT {
                return vec![Action::Close];
            }
            for (id, task) in &self.tasks {
                if task.generation == self.generation && self.stopped.insert(id.clone()) {
                    actions.push(Action::Stop(id.clone()));
                }
            }
            if self.root_live && !self.interrupt_sent {
                self.interrupt_sent = true;
                actions.push(Action::Interrupt);
            }
        }
        let occupied = self
            .tasks
            .values()
            .any(|t| t.generation == self.generation && (self.cancelling() || t.holds_turn));
        if self.root_live || occupied || pending_agent_interactions || self.terminal.is_none() {
            self.quiet_since = None;
            return actions;
        }
        if self.held || self.cancelling() {
            let quiet = self.quiet_since.get_or_insert(now);
            if now.duration_since(*quiet) < QUIET {
                return actions;
            }
        }
        let event = self.terminal.take().expect("checked terminal");
        actions.push(Action::Finish(if self.cancelling() {
            DriverEvent::TurnInterrupted {
                summary: "Claude Code turn was interrupted".into(),
            }
        } else {
            event
        }));
        self.cancel_since = None;
        actions
    }
}

impl super::SessionProcess {
    async fn send_bounded(&self, value: &Value) -> Result<(), driver_api::DriverError> {
        tokio::time::timeout(Duration::from_millis(300), self.send_line(value))
            .await
            .map_err(|_| {
                driver_api::DriverError::Disconnected("Claude control write timed out".into())
            })?
    }

    pub(super) async fn send_control(&self, request: Value) -> Result<(), driver_api::DriverError> {
        self.send_bounded(&serde_json::json!({"type":"control_request","request_id":uuid::Uuid::new_v4().to_string(),"request":request})).await
    }

    pub(super) async fn deny_interaction(&self, id: &str) -> Result<(), driver_api::DriverError> {
        self.send_bounded(&serde_json::json!({"type":"control_response","response":{"subtype":"success","request_id":id,"response":{"behavior":"deny","message":"Claude interaction is no longer pending"}}})).await
    }

    pub(super) fn expire_interaction(&self, id: &str) {
        let question = self
            .pending_questions
            .lock()
            .expect("questions")
            .remove(id)
            .is_some();
        let approval = self
            .pending_approvals
            .lock()
            .expect("approvals")
            .remove(id)
            .is_some();
        self.agent_interactions
            .lock()
            .expect("agent interactions")
            .remove(id);
        if (question || approval)
            && let Some(sink) = self.updates.lock().expect("updates").as_ref()
        {
            let _ = sink.send(if question {
                DriverEvent::QuestionClosed {
                    native_request_id: id.into(),
                }
            } else {
                DriverEvent::ApprovalClosed {
                    native_request_id: id.into(),
                }
            });
        }
    }

    pub(super) async fn close_interactions(&self, all: bool) {
        let ids: Vec<_> = self
            .pending_questions
            .lock()
            .expect("questions")
            .keys()
            .cloned()
            .chain(
                self.pending_approvals
                    .lock()
                    .expect("approvals")
                    .keys()
                    .cloned(),
            )
            .collect();
        for id in ids {
            if !all
                && self
                    .agent_interactions
                    .lock()
                    .expect("agent interactions")
                    .contains_key(&id)
            {
                continue;
            }
            self.expire_interaction(&id);
            let _ = self.deny_interaction(&id).await;
        }
    }

    pub(super) async fn close_agent_interactions(&self, task: Option<&str>, all: bool) {
        let ids: Vec<_> = self
            .agent_interactions
            .lock()
            .expect("agent interactions")
            .iter()
            .filter(|(_, agent)| all || task == Some(agent.as_str()))
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            self.expire_interaction(&id);
            let _ = self.deny_interaction(&id).await;
        }
    }

    pub(super) fn finish_turn(&self, event: DriverEvent) {
        use std::sync::atomic::Ordering;
        let mut sink = self.updates.lock().expect("updates");
        self.turn_active.store(false, Ordering::Release);
        self.interrupt_requested.store(false, Ordering::Release);
        if let Some(sink) = sink.take() {
            let _ = sink.send(event);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tool_results_track_background_handoffs_and_ignore_duplicate_starts() {
        let mut state = Lifecycle::default();
        state.begin();
        state.observe(&json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":"w","name":"Workflow"}]}}));
        state.observe(&json!({"type":"user","tool_use_result":{"status":"async_launched","taskId":"task"},"message":{"content":[{"type":"tool_result","tool_use_id":"w"}]}}));
        state.result(DriverEvent::TurnCompleted {
            summary: "root".into(),
        });
        assert!(state.poll(false).is_empty());
        state.cancel();
        assert!(matches!(state.poll(false).as_slice(), [Action::Stop(id)] if id == "task"));
        assert!(
            state.poll(false).is_empty(),
            "do not resend stops after an ACK"
        );
        state.observe(&json!({"type":"system","subtype":"task_notification","task_id":"task","status":"stopped"}));
        state.observe(
            &json!({"type":"system","subtype":"task_started","task_id":"task","tool_use_id":"w"}),
        );
        assert!(state.tasks.is_empty());
        state.quiet_since = Some(Instant::now() - QUIET);
        assert!(matches!(
            state.poll(false).as_slice(),
            [Action::Finish(DriverEvent::TurnInterrupted { .. })]
        ));
    }

    #[test]
    fn prior_background_bash_is_not_cancelled_with_the_next_turn() {
        let mut state = Lifecycle::default();
        state.begin();
        state.observe(&json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":"b","name":"Bash"}]}}));
        state.observe(&json!({"type":"user","tool_use_result":{"backgroundTaskId":"shell"},"message":{"content":[{"type":"tool_result","tool_use_id":"b"}]}}));
        state.result(DriverEvent::TurnCompleted {
            summary: "root".into(),
        });
        assert!(matches!(state.poll(false).as_slice(), [Action::Finish(_)]));
        state.begin();
        state.cancel();
        assert!(matches!(state.poll(false).as_slice(), [Action::Interrupt]));
        state.observe(&json!({"type":"system","subtype":"task_started","task_id":"new-task","task_type":"local_agent"}));
        assert!(matches!(state.poll(false).as_slice(), [Action::Stop(id)] if id == "new-task"));
    }

    #[test]
    fn missing_native_confirmation_requires_process_shutdown() {
        let mut state = Lifecycle::default();
        state.begin();
        state.cancel();
        state.cancel_since = Some(Instant::now() - CANCEL_TIMEOUT);
        assert!(matches!(state.poll(false).as_slice(), [Action::Close]));
    }
}
