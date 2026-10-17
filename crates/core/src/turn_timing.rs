//! Monotonic, content-free diagnostics, separate from canonical task events.
use agent_domain::{CommandId, RunId, TaskId};
use serde::Serialize;
use std::{sync::Arc, time::Instant};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnTimingRecord {
    pub recorded_at: chrono::DateTime<chrono::Utc>,
    pub trace_id: CommandId,
    pub task_id: TaskId,
    pub run_id: Option<RunId>,
    pub stage: &'static str,
    pub event: &'static str,
    pub elapsed_ms: f64,
    pub duration_ms: Option<f64>,
}
pub type TurnTimingSink = Arc<dyn Fn(TurnTimingRecord) + Send + Sync>;

pub(crate) fn has_body_text(event: &driver_api::DriverEvent) -> bool {
    use driver_api::DriverEvent;
    match event {
        DriverEvent::TextDelta { text, .. } => !text.is_empty(),
        DriverEvent::Presentation { data, .. } if data["type"] == "message" => {
            ["text", "textDelta"]
                .iter()
                .any(|key| data[key].as_str().is_some_and(|text| !text.is_empty()))
        }
        _ => false,
    }
}

#[derive(Clone)]
pub(crate) struct TurnStartupTiming {
    started: Instant,
    trace_id: CommandId,
    task_id: TaskId,
    run_id: Option<RunId>,
    sink: Option<TurnTimingSink>,
}
impl TurnStartupTiming {
    pub(crate) fn elapsed_ms(&self) -> u64 {
        u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX)
    }
    pub(crate) fn new(task_id: TaskId, sink: Option<TurnTimingSink>) -> Self {
        Self {
            started: Instant::now(),
            trace_id: CommandId::new(),
            task_id,
            run_id: None,
            sink,
        }
    }
    pub(crate) fn bind_run(&mut self, run_id: RunId) {
        self.run_id = Some(run_id);
    }
    pub(crate) fn mark(&self, stage: &'static str) {
        self.emit(stage, "mark", None);
    }
    fn emit(&self, stage: &'static str, event: &'static str, duration_ms: Option<f64>) {
        if let Some(sink) = &self.sink {
            sink(TurnTimingRecord {
                recorded_at: chrono::Utc::now(),
                trace_id: self.trace_id.clone(),
                task_id: self.task_id.clone(),
                run_id: self.run_id.clone(),
                stage,
                event,
                elapsed_ms: self.started.elapsed().as_secs_f64() * 1000.0,
                duration_ms,
            });
        }
    }
    pub(crate) fn stage(&self, name: &'static str) -> Stage {
        self.emit(name, "begin", None);
        Stage {
            timing: self.clone(),
            name,
            started: Instant::now(),
        }
    }
}
pub(crate) struct Stage {
    timing: TurnStartupTiming,
    name: &'static str,
    started: Instant,
}
impl Drop for Stage {
    fn drop(&mut self) {
        self.timing.emit(
            self.name,
            "end",
            Some(self.started.elapsed().as_secs_f64() * 1000.0),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recognizes_acp_messages_but_not_thoughts_or_empty_chunks() {
        use driver_api::DriverEvent;
        for key in ["text", "textDelta"] {
            assert!(has_body_text(&DriverEvent::Presentation {
                item_id: "m".into(),
                data: serde_json::json!({"type":"message",key:"hello"})
            }));
        }
        assert!(!has_body_text(&DriverEvent::Presentation {
            item_id: "t".into(),
            data: serde_json::json!({"type":"thought","text":"thinking"})
        }));
        assert!(!has_body_text(&DriverEvent::TextDelta {
            item_id: "m".into(),
            text: String::new()
        }));
    }
    #[test]
    fn correlated_monotonic_stages_close_on_early_return() {
        let records = Arc::new(std::sync::Mutex::new(Vec::new()));
        let output = records.clone();
        let mut timing = TurnStartupTiming::new(
            TaskId::new(),
            Some(Arc::new(move |record| output.lock().unwrap().push(record))),
        );
        let run = RunId::new();
        timing.bind_run(run.clone());
        let fail = || -> Result<(), ()> {
            let _stage = timing.stage("native_session");
            Err(())
        };
        assert!(fail().is_err());
        timing.mark("startup_failed");
        let records = records.lock().unwrap();
        assert_eq!(records.len(), 3);
        assert_eq!(records[0].event, "begin");
        assert_eq!(records[1].event, "end");
        assert!(records[1].duration_ms.is_some());
        assert!(
            records
                .windows(2)
                .all(|pair| pair[0].elapsed_ms <= pair[1].elapsed_ms)
        );
        assert!(
            records
                .iter()
                .all(|record| record.run_id.as_ref() == Some(&run))
        );
    }
}
