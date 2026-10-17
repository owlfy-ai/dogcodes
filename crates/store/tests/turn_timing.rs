#[path = "support/installations.rs"]
mod installations;
use agent_core::{Core, turn_timing::TurnTimingRecord};
use agent_domain::{ProjectId, TaskId};
use agent_protocol::{CommandEnvelope, CoreCommand, CreateTask, RegisterProject, SubmitTurn};
use agent_store::SqliteStore;
use driver_api::{AgentDriver, FakeDriver};
use std::sync::{Arc, Mutex};

#[tokio::test]
async fn startup_trace_distinguishes_preparation_dispatch_first_text_and_failure() {
    let records = Arc::new(Mutex::new(Vec::<TurnTimingRecord>::new()));
    let output = records.clone();
    let core = Arc::new(
        Core::new(Arc::new(SqliteStore::open_in_memory().unwrap()))
            .with_turn_timing_sink(Arc::new(move |record| output.lock().unwrap().push(record))),
    );
    let driver = Arc::new(FakeDriver::with_executor(
        installations::probe(&core, "fake").await,
    ));
    core.register_driver(driver.clone()).await;
    let project = ProjectId::new();
    let task = TaskId::new();
    core.execute(CommandEnvelope::local(CoreCommand::RegisterProject(
        RegisterProject {
            project_id: project.clone(),
            display_name: "timing".into(),
            path: "/tmp".into(),
            additional_directories: vec![],
        },
    )))
    .await
    .unwrap();
    core.execute(CommandEnvelope::local(CoreCommand::CreateTask(
        CreateTask {
            task_id: task.clone(),
            project_id: project,
            title: "timing".into(),
            preferred_executor: driver.executor().clone(),
        },
    )))
    .await
    .unwrap();
    for fail in [false, true] {
        records.lock().unwrap().clear();
        if fail {
            driver.fail_next_prompt();
        }
        let mut command = CommandEnvelope::local(CoreCommand::SubmitTurn(SubmitTurn {
            task_id: task.clone(),
            prompt: "private prompt must never enter diagnostic records".into(),
            review_fix: None,
            attachments: vec![],
            model: None,
            thinking: None,
            permission_mode: None,
            executor_override: None,
        }));
        command.expected_stream_version =
            Some(core.bootstrap().await.unwrap().tasks[0].stream_version);
        core.execute(command).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while core.bootstrap().await.unwrap().tasks[0]
                .active_primary_run
                .is_some()
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let rows = records.lock().unwrap();
        assert!(
            rows.iter()
                .all(|row| row.task_id == task && row.trace_id == rows[0].trace_id)
        );
        assert!(
            rows.windows(2)
                .all(|pair| pair[0].elapsed_ms <= pair[1].elapsed_ms)
        );
        for stage in [
            "workspace_capture_begin",
            "workspace_validation",
            "history_bridge",
            "native_session",
            "native_prompt_dispatch",
        ] {
            assert!(
                rows.iter().any(|row| row.stage == stage
                    && row.event == "end"
                    && row.duration_ms.is_some()),
                "{stage}"
            );
        }
        let marks: Vec<_> = rows
            .iter()
            .filter(|row| row.event == "mark")
            .map(|row| row.stage)
            .collect();
        if fail {
            assert!(marks.contains(&"run_failed"));
            assert!(!marks.contains(&"first_text"));
        } else {
            assert_eq!(
                marks,
                [
                    "turn_queued",
                    "prompt_accepted",
                    "first_driver_event",
                    "first_text",
                    "first_text_published",
                    "run_stream_finished"
                ]
            );
        }
        assert!(
            !serde_json::to_string(&*rows)
                .unwrap()
                .contains("private prompt")
        );
    }
}
