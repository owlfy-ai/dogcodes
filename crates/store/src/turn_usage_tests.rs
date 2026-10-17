use super::*;
use driver_api::{
    QuotaProvider, QuotaSnapshot,
    usage::{RequestUsageReport, SessionUsageSnapshot, UsageRequest},
};

struct UsageProvider {
    executor: ExecutorRef,
    reads: std::sync::atomic::AtomicUsize,
    source: &'static str,
}

struct SettledUsageProvider {
    executor: ExecutorRef,
    reads: std::sync::atomic::AtomicUsize,
    records: Vec<UsageRequest>,
}
#[async_trait::async_trait]
impl QuotaProvider for SettledUsageProvider {
    fn executor(&self) -> &ExecutorRef {
        &self.executor
    }
    async fn read_quota(&self, _: Option<&str>) -> Result<QuotaSnapshot, DriverError> {
        panic!("not account quota")
    }
    async fn read_session_usage(&self, _: &str) -> Option<SessionUsageSnapshot> {
        let settled = self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst) > 0;
        Some(SessionUsageSnapshot {
            request_usage: Some(RequestUsageReport {
                records: if settled {
                    self.records.clone()
                } else {
                    vec![]
                },
                ..Default::default()
            }),
            native_summary_id: Some(if settled { "new" } else { "old" }.into()),
            totals_scope: Some("turn".into()),
            cost_usd: Some(if settled { 0.03 } else { 1.0 }),
            session_cost_usd: Some(10.0),
            ..Default::default()
        })
    }
}

#[tokio::test]
async fn request_corrections_and_settled_turn_cost_survive_failure_and_projection_rebuild() {
    let store = Arc::new(SqliteStore::open_in_memory().unwrap());
    let core = Arc::new(Core::new(store.clone()));
    let live: Vec<_> = (0..2)
        .map(|n| UsageRequest {
            request_id: format!("response-{n}"),
            source: "codex.response".into(),
            model: Some("requested".into()),
            input_tokens: Some(100),
            output_tokens: Some(10),
            generation_ms: Some(100),
            ..Default::default()
        })
        .collect();
    let corrected = live
        .iter()
        .cloned()
        .map(|mut r| {
            r.generation_ms = None;
            r.model = Some("actual".into());
            r
        })
        .collect();
    let mut events = live
        .into_iter()
        .map(|request| DriverEvent::UsageRecorded { request })
        .collect::<Vec<_>>();
    events.push(DriverEvent::TurnFailed {
        reason: "later request failed".into(),
    });
    let driver = Arc::new(ScriptedDriver::new("scripted", events));
    core.register_quota_provider(Arc::new(SettledUsageProvider {
        executor: driver.executor().clone(),
        reads: std::sync::atomic::AtomicUsize::new(0),
        records: corrected,
    }))
    .await;
    fixture_installations::register(&core, store.as_ref(), driver).await;
    let task = scripted_task(&core).await;
    submit_turn(&core, &task, "test").await;
    wait_until_idle(&core).await;
    let timeline = core.timeline(&task).await.unwrap();
    let usage = timeline[0]
        .turn_completion
        .as_ref()
        .unwrap()
        .usage
        .as_ref()
        .unwrap();
    assert_eq!(usage.request_count, Some(2));
    assert_eq!(usage.output_tokens_per_second, Some(100.0));
    assert_eq!(usage.models, vec!["actual"]);
    assert_eq!(usage.cost_usd, Some(0.03));
    store.rebuild_projections().unwrap();
    assert_eq!(core.timeline(&task).await.unwrap(), timeline);
}

#[tokio::test]
async fn rejected_selection_preserves_the_previous_mapping_and_never_dispatches_prompt() {
    let store = Arc::new(SqliteStore::open_in_memory().unwrap());
    let core = Arc::new(Core::new(store.clone()));
    let mut fixture = ScriptedDriver::new(
        "scripted",
        vec![DriverEvent::TurnCompleted {
            summary: "done".into(),
        }],
    );
    fixture.reuse = true;
    fixture.selection_failure = Some(DriverError::Protocol("selection_rejected".into()));
    let driver = Arc::new(fixture);
    fixture_installations::register(&core, store.as_ref(), driver.clone()).await;
    let task = scripted_task(&core).await;
    submit_turn(&core, &task, "first").await;
    wait_until_idle(&core).await;
    let before = core.bootstrap().await.unwrap();
    let mut submit = CommandEnvelope::local(CoreCommand::SubmitTurn(SubmitTurn {
        task_id: task.clone(),
        prompt: "must not dispatch".into(),
        attachments: vec![],
        executor_override: None,
        model: Some("other-model".into()),
        thinking: None,
        permission_mode: None,
        review_fix: None,
    }));
    submit.expected_stream_version = Some(before.tasks[0].stream_version);
    core.execute(submit).await.unwrap();
    wait_until_idle(&core).await;
    let after = core.bootstrap().await.unwrap();
    assert_eq!(after.session_maps[0].model, before.session_maps[0].model);
    assert_eq!(
        after.session_maps[0].session_map_id,
        before.session_maps[0].session_map_id
    );
    assert_eq!(
        after.session_maps[0].status,
        agent_protocol::SessionMapStatus::Active
    );
    assert_eq!(driver.contexts.lock().unwrap().len(), 1);
    assert!(
        core.timeline(&task)
            .await
            .unwrap()
            .iter()
            .any(|i| i.text.contains("已恢复原选择"))
    );
}
#[async_trait::async_trait]
impl QuotaProvider for UsageProvider {
    fn executor(&self) -> &ExecutorRef {
        &self.executor
    }
    async fn read_quota(&self, _: Option<&str>) -> Result<QuotaSnapshot, DriverError> {
        panic!("turn accounting must not query account quota")
    }
    async fn read_session_usage(&self, _: &str) -> Option<SessionUsageSnapshot> {
        let read = self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let count = read.div_ceil(2);
        Some(SessionUsageSnapshot {
            request_usage: Some(RequestUsageReport {
                records: (0..count)
                    .map(|n| UsageRequest {
                        request_id: format!("request-{n}"),
                        source: self.source.into(),
                        input_tokens: Some(if n == 0 { 100 } else { 20 }),
                        output_tokens: Some(5),
                        ..UsageRequest::default()
                    })
                    .collect(),
                ..RequestUsageReport::default()
            }),
            ..SessionUsageSnapshot::new()
        })
    }
}

#[tokio::test]
async fn turn_usage_is_scoped_persisted_and_rebuilt_for_completed_and_failed_turns() {
    for terminal in [
        DriverEvent::TurnCompleted {
            summary: "done".into(),
        },
        DriverEvent::TurnFailed {
            reason: "interrupted".into(),
        },
    ] {
        let store = Arc::new(SqliteStore::open_in_memory().unwrap());
        let core = Arc::new(Core::new(store.clone()));
        let mut driver = ScriptedDriver::new("scripted", vec![terminal]);
        driver.reuse = true;
        fixture_installations::register(&core, store.as_ref(), Arc::new(driver)).await;
        let provider = Arc::new(UsageProvider {
            executor: executor("scripted"),
            reads: std::sync::atomic::AtomicUsize::new(0),
            source: "",
        });
        core.register_quota_provider(provider).await;
        let task = scripted_task(&core).await;
        submit_turn(&core, &task, "first").await;
        wait_until_idle(&core).await;
        let first = core.timeline(&task).await.unwrap();
        let first_turn = first[0].turn_id.clone().unwrap();
        assert_eq!(
            first[0]
                .turn_completion
                .as_ref()
                .unwrap()
                .usage
                .as_ref()
                .unwrap()
                .input_tokens,
            Some(100)
        );
        submit_turn(&core, &task, "second").await;
        wait_until_idle(&core).await;
        let timeline = core.timeline(&task).await.unwrap();
        for item in &timeline {
            let stats = item
                .turn_completion
                .as_ref()
                .unwrap()
                .usage
                .as_ref()
                .unwrap();
            assert_eq!(stats.request_count, Some(1));
            assert_eq!(
                stats.input_tokens,
                Some(if item.turn_id.as_ref() == Some(&first_turn) {
                    100
                } else {
                    20
                })
            );
        }
        assert_eq!(
            timeline
                .iter()
                .filter(|item| item.turn_id.as_ref() == Some(&first_turn))
                .cloned()
                .collect::<Vec<_>>(),
            first
        );
        store.rebuild_projections().unwrap();
        let restarted = Core::new(store.clone());
        assert_eq!(restarted.timeline(&task).await.unwrap(), timeline);
    }
}

#[tokio::test]
async fn turn_usage_live_facts_recover_without_final_summary_and_deduplicate_after_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("usage.sqlite");
    let store = Arc::new(SqliteStore::open(&path).unwrap());
    let core = Arc::new(Core::new(store.clone()));
    let request = UsageRequest {
        request_id: "native-response".into(),
        input_tokens: Some(40),
        output_tokens: Some(20),
        generation_ms: Some(500),
        ..UsageRequest::default()
    };
    fixture_installations::register(
        &core,
        store.as_ref(),
        Arc::new(ScriptedDriver::new(
            "scripted",
            vec![
                DriverEvent::FirstOutput,
                DriverEvent::UsageRecorded {
                    request: request.clone(),
                },
                DriverEvent::UsageRecorded { request },
                DriverEvent::TextDelta {
                    item_id: "answer".into(),
                    text: "done".into(),
                },
                DriverEvent::TurnFailed {
                    reason: "interrupted".into(),
                },
            ],
        )),
    )
    .await;
    let task = scripted_task(&core).await;
    submit_turn(&core, &task, "first").await;
    wait_until_idle(&core).await;
    let original = core.timeline(&task).await.unwrap();
    let completion = original[0].turn_completion.as_ref().unwrap();
    assert!(completion.first_output_ms.is_some());
    let usage = completion.usage.as_ref().unwrap();
    assert_eq!(usage.request_count, Some(1));
    assert_eq!(usage.output_tokens_per_second, Some(40.0));
    // The optional final summary can be absent after a crash. Durable facts suffice.
    store
        .connection
        .lock()
        .unwrap()
        .execute(
            "DELETE FROM events WHERE event_type='turn.usage_recorded'",
            [],
        )
        .unwrap();
    drop(core);
    drop(store);
    let reopened = Arc::new(SqliteStore::open(path).unwrap());
    reopened.rebuild_projections().unwrap();
    assert_eq!(Core::new(reopened).timeline(&task).await.unwrap(), original);
}

#[tokio::test]
async fn turn_usage_autonomous_continuations_keep_their_own_requests_and_timing() {
    let store = Arc::new(SqliteStore::open_in_memory().unwrap());
    let core = Arc::new(Core::new(store.clone()));
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut driver = ScriptedDriver::new(
        "scripted",
        vec![DriverEvent::TurnCompleted {
            summary: "done".into(),
        }],
    );
    driver.reuse = true;
    *driver.autonomous.lock().unwrap() =
        Some(Box::pin(futures::stream::unfold(rx, |mut rx| async {
            rx.recv().await.map(|event| (event, rx))
        })));
    fixture_installations::register(&core, store.as_ref(), Arc::new(driver)).await;
    let task = scripted_task(&core).await;
    submit_turn(&core, &task, "initial").await;
    wait_until_idle(&core).await;
    for n in 1..=2 {
        tx.send(Ok(DriverEvent::TurnStarted {
            native_turn_id: Some(format!("auto-{n}")),
        }))
        .unwrap();
        tx.send(Ok(DriverEvent::FirstOutput)).unwrap();
        tx.send(Ok(DriverEvent::UsageRecorded {
            request: UsageRequest {
                request_id: format!("request-{n}"),
                input_tokens: Some(n * 10),
                output_tokens: Some(n),
                generation_ms: Some(100),
                ..UsageRequest::default()
            },
        }))
        .unwrap();
        tx.send(Ok(DriverEvent::TextDelta {
            item_id: "answer".into(),
            text: format!("auto {n}"),
        }))
        .unwrap();
        tx.send(Ok(DriverEvent::TurnCompleted {
            summary: "done".into(),
        }))
        .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                let items = core.timeline(&task).await.unwrap();
                if let Some(item) = items
                    .iter()
                    .find(|i| i.text == format!("auto {n}") && i.turn_completion.is_some())
                {
                    let completion = item.turn_completion.as_ref().unwrap();
                    assert!(completion.first_output_ms.is_some());
                    let usage = completion.usage.as_ref().unwrap();
                    assert_eq!(usage.request_count, Some(1));
                    assert_eq!(usage.input_tokens, Some(n * 10));
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
    let timeline = core.timeline(&task).await.unwrap();
    store.rebuild_projections().unwrap();
    assert_eq!(core.timeline(&task).await.unwrap(), timeline);
}

#[tokio::test]
async fn native_debug_usage_never_double_counts_projection_and_survives_debug_outage() {
    let store = Arc::new(SqliteStore::open_in_memory().unwrap());
    let core = Arc::new(Core::new(store.clone()));
    core.register_quota_provider(Arc::new(UsageProvider {
        executor: executor("scripted"),
        reads: std::sync::atomic::AtomicUsize::new(0),
        source: "zcode.session.messages",
    }))
    .await;
    let task = scripted_task(&core).await;
    for (index, tokens) in [Some(100), None, Some(30)].into_iter().enumerate() {
        let mut script = vec![];
        if let Some(input) = tokens {
            script.push(DriverEvent::UsageRecorded {
                request: UsageRequest {
                    request_id: format!("native-debug-{index}"),
                    source: "zcode.session.debug".into(),
                    input_tokens: Some(input),
                    output_tokens: Some(5),
                    generation_ms: Some(1000),
                    ..UsageRequest::default()
                },
            });
        }
        script.push(DriverEvent::TurnCompleted {
            summary: "done".into(),
        });
        let mut driver = ScriptedDriver::new("scripted", script);
        driver.reuse = true;
        fixture_installations::register(&core, store.as_ref(), Arc::new(driver)).await;
        submit_turn(&core, &task, "next").await;
        wait_until_idle(&core).await;
    }
    let timeline = core.timeline(&task).await.unwrap();
    let turns = timeline
        .iter()
        .filter_map(|item| item.turn_completion.as_ref())
        .filter_map(|completion| completion.usage.as_ref())
        .collect::<Vec<_>>();
    assert_eq!(turns.len(), 3);
    for (index, turn) in turns.iter().enumerate() {
        assert_eq!(turn.request_count, Some(1));
        assert_eq!(turn.input_tokens, Some([100, 20, 30][index]));
        assert_eq!(
            turn.output_tokens_per_second,
            if index == 1 { None } else { Some(5.0) }
        );
    }
    store.rebuild_projections().unwrap();
    assert_eq!(Core::new(store).timeline(&task).await.unwrap(), timeline);
}
