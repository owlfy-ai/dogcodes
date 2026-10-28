//! Real private ACP runtimes, never installed agents or model requests.
use super::*;
trait Bounded: futures::Stream + Sized {
    async fn finish_bounded(self) -> Vec<Self::Item> {
        tokio::time::timeout(Duration::from_secs(5), self.collect::<Vec<_>>())
            .await
            .expect("synthetic ACP turn did not finish")
    }
}
impl<S: futures::Stream + Sized> Bounded for S {}
fn requests(root: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(root.join("requests.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|s| serde_json::from_str(s).ok())
        .collect()
}
fn method_pids(root: &Path, method: &str) -> Vec<u32> {
    requests(root)
        .into_iter()
        .filter(|r| r["method"] == method)
        .filter_map(|r| r["pid"].as_u64().map(|p| u32::try_from(p).unwrap()))
        .collect()
}
#[tokio::test]
async fn account_refresh_uses_disposable_scopes_and_preserves_cache_and_auth_invalidation() {
    let (root, driver) = fixture_as("aux-account", "kiro");
    let (a, b) = tokio::join!(driver.read_quota(None), driver.read_quota(None));
    assert_eq!(a.unwrap(), b.unwrap());
    assert_eq!(count(root.path(), "_kiro/account/getUsage"), 1);
    assert!(driver.shared.connection.lock().await.is_none());
    let fresh = driver.read_quota_with_refresh(None, true).await.unwrap();
    assert_eq!(count(root.path(), "_kiro/account/getUsage"), 2);
    std::fs::write(root.path().join("quota-failure"), "fixture offline").unwrap();
    assert!(driver.read_quota_with_refresh(None, true).await.is_err());
    assert_eq!(driver.read_quota(None).await.unwrap(), fresh);
    assert_eq!(count(root.path(), "_kiro/account/getUsage"), 3);
    std::fs::write(root.path().join("quota-failure"), "authentication_required").unwrap();
    assert!(matches!(
        driver.read_quota_with_refresh(None, true).await,
        Err(DriverError::NotConfigured(_))
    ));
    assert!(matches!(
        driver.read_quota(None).await,
        Err(DriverError::NotConfigured(_))
    ));
    assert!(driver.shared.quota.account_snapshot(None).is_none());
    let pids = method_pids(root.path(), "_kiro/account/getUsage");
    assert_eq!(pids.len(), 4);
    assert!(
        pids.iter().all(|id| !present(*id)),
        "quota result preceded helper exit"
    );
    assert_eq!(count(root.path(), "session/new"), 0);
    assert_eq!(count(root.path(), "session/prompt"), 0);
}
#[tokio::test]
async fn account_queries_during_business_turns_cannot_route_helper_updates_or_replace_the_process()
{
    for provider in ["kiro", "grok"] {
        let (root, driver) = fixture_as("aux-account", provider);
        let a = driver.create_session(request(root.path())).await.unwrap();
        let (_, epoch) = original(&driver).await;
        let business = ids(root.path());
        let stream = driver
            .prompt(&a, prompt(root.path(), "slow"))
            .await
            .unwrap();
        wait_for(|| count(root.path(), "session/prompt") == 1).await;
        driver.read_quota_with_refresh(None, true).await.unwrap();
        let method = if provider == "kiro" {
            "_kiro/account/getUsage"
        } else {
            "_x.ai/billing"
        };
        let helper = method_pids(root.path(), method);
        assert_eq!(helper.len(), 1);
        assert!(!business.contains(&helper[0]));
        assert!(!present(helper[0]));
        assert!(business.iter().all(|id| present(*id)));
        assert!(!epoch.load(Ordering::Acquire));
        assert_eq!(count(root.path(), "session/new"), 1);
        driver.interrupt(&a).await.unwrap();
        let events = stream.finish_bounded().await;
        assert!(!events.iter().any(
            |e| matches!(e,Ok(DriverEvent::TextDelta{text,..}) if text.contains("quota-only"))
        ));
        driver.shutdown(a).await.unwrap();
    }
}
#[tokio::test]
async fn cancelled_account_waiters_stop_only_their_helper_and_keep_admission_through_exit() {
    let (root, driver) = fixture_as("aux-account-blocked", "kiro");
    let a = driver.create_session(request(root.path())).await.unwrap();
    let business = ids(root.path());
    let refreshing = tokio::spawn({
        let d = driver.clone();
        async move { d.read_quota(None).await }
    });
    wait_for(|| count(root.path(), "_kiro/account/getUsage") == 1).await;
    let helper = method_pids(root.path(), "_kiro/account/getUsage")[0];
    refreshing.abort();
    let _ = refreshing.await;
    wait_for(|| !present(helper)).await;
    wait_for(|| driver.shared.metadata.try_lock().is_ok()).await;
    assert!(business.iter().all(|id| present(*id)));
    driver.shutdown(a).await.unwrap();
}
#[tokio::test]
async fn cancelled_context_waiter_retains_one_request_and_blocks_only_ambiguous_callbacks() {
    let (root, driver) = fixture_as("aux-context", "kiro");
    let a = driver.create_session(request(root.path())).await.unwrap();
    let b = driver.create_session(request(root.path())).await.unwrap();
    let query = tokio::spawn({
        let d = driver.clone();
        let id = a.native_session_id.clone();
        async move { d.refresh_kiro_context(&id).await }
    });
    wait_for(|| count(root.path(), "_kiro/session/context") == 1).await;
    query.abort();
    let _ = query.await;
    assert_eq!(driver.shared.pending_auxiliary.load(Ordering::Acquire), 1);
    driver.refresh_kiro_context(&a.native_session_id).await;
    assert_eq!(
        count(root.path(), "_kiro/session/context"),
        1,
        "cancelled read was replayed"
    );
    let events = driver
        .prompt(&b, prompt(root.path(), "ambiguous"))
        .await
        .unwrap()
        .finish_bounded()
        .await;
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Ok(DriverEvent::QuestionRequested { .. })))
    );
    let mut stream = driver
        .prompt(&b, prompt(root.path(), "scoped"))
        .await
        .unwrap();
    let question = question_id(&mut stream).await;
    driver
        .respond_to_question(
            &b,
            &question,
            agent_domain::QuestionAnswer {
                cancelled: true,
                values: serde_json::Value::Null,
            },
        )
        .await
        .unwrap();
    let _ = stream.finish_bounded().await;
    std::fs::write(root.path().join("context-ready"), b"").unwrap();
    wait_for(|| driver.shared.pending_auxiliary.load(Ordering::Acquire) == 0).await;
    assert!(
        driver
            .shared
            .quota
            .usage
            .get(Some(&a.native_session_id))
            .and_then(|u| u.context_used_percent)
            .is_none()
    );
    driver.shutdown(a).await.unwrap();
    driver.shutdown(b).await.unwrap();
}
#[tokio::test]
async fn late_context_cannot_overwrite_a_reloaded_same_native_id_or_return_its_new_usage() {
    let (root, driver) = fixture_as("aux-context", "kiro");
    let a = driver.create_session(request(root.path())).await.unwrap();
    let b = driver.create_session(request(root.path())).await.unwrap();
    let read = tokio::spawn({
        let d = driver.clone();
        let id = a.native_session_id.clone();
        async move { d.read_session_usage(&id).await }
    });
    wait_for(|| count(root.path(), "_kiro/session/context") == 1).await;
    driver.shutdown(a.clone()).await.unwrap();
    assert_eq!(
        driver.shared.pending_auxiliary.load(Ordering::Acquire),
        1,
        "local retirement is not native query completion"
    );
    let a = driver
        .resume_session(&a.native_session_id, &root.path().to_string_lossy())
        .await
        .unwrap();
    driver.shared.quota.observe_kiro_context(
        &a.native_session_id,
        &serde_json::json!({"usagePercentage":22}),
    );
    std::fs::write(root.path().join("context-ready"), b"").unwrap();
    assert!(read.await.unwrap().is_none());
    wait_for(|| driver.shared.pending_auxiliary.load(Ordering::Acquire) == 0).await;
    assert_eq!(
        driver
            .shared
            .quota
            .usage
            .get(Some(&a.native_session_id))
            .unwrap()
            .context_used_percent,
        Some(22.0)
    );
    driver.shutdown(a).await.unwrap();
    driver.shutdown(b).await.unwrap();
}
#[tokio::test]
async fn native_close_proves_an_unanswered_context_finished_without_stopping_peer_work() {
    let (root, driver) = fixture_as("native-close-aux", "kiro");
    let a = driver.create_session(request(root.path())).await.unwrap();
    let b = driver.create_session(request(root.path())).await.unwrap();
    let (_, epoch) = original(&driver).await;
    let read = tokio::spawn({
        let d = driver.clone();
        let id = a.native_session_id.clone();
        async move { d.read_session_usage(&id).await }
    });
    wait_for(|| count(root.path(), "_kiro/session/context") == 1).await;
    driver.shutdown(a).await.unwrap();
    let _ = read.await.unwrap();
    wait_for(|| driver.shared.pending_auxiliary.load(Ordering::Acquire) == 0).await;
    assert!(!epoch.load(Ordering::Acquire));
    std::fs::write(root.path().join("context-ready"), b"").unwrap();
    let events = driver
        .prompt(&b, prompt(root.path(), "peer"))
        .await
        .unwrap()
        .finish_bounded()
        .await;
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Ok(DriverEvent::TurnCompleted { .. })))
    );
    assert_eq!(count(root.path(), "initialize"), 1);
    driver.shutdown(b).await.unwrap();
}
#[tokio::test]
async fn a_context_caller_cancelled_in_the_sdk_queue_never_dispatches_later() {
    let (root, driver) = fixture_as("blocked-catalog", "kiro");
    let a = driver.create_session(request(root.path())).await.unwrap();
    let b = driver.create_session(request(root.path())).await.unwrap();
    let blocked = original_catalog_barrier(&driver).await;
    wait_for(|| count(root.path(), "session/new") == 3).await;
    let read = tokio::spawn({
        let d = driver.clone();
        let id = a.native_session_id.clone();
        async move { d.refresh_kiro_context(&id).await }
    });
    wait_for(|| driver.shared.pending_auxiliary.load(Ordering::Acquire) == 1).await;
    read.abort();
    let _ = read.await;
    std::fs::write(root.path().join("metadata-ready"), b"").unwrap();
    let _ = blocked.await;
    wait_for(|| driver.shared.pending_auxiliary.load(Ordering::Acquire) == 0).await;
    assert_eq!(count(root.path(), "_kiro/session/context"), 0);
    driver.shutdown(a).await.unwrap();
    driver.shutdown(b).await.unwrap();
}
#[tokio::test]
async fn cursor_task_callbacks_cannot_consume_another_active_sessions_tool() {
    let (root, driver) = fixture("normal");
    let a = driver.create_session(request(root.path())).await.unwrap();
    let b = driver.create_session(request(root.path())).await.unwrap();
    let events = driver
        .prompt(&b, prompt(root.path(), "foreign-task"))
        .await
        .unwrap()
        .finish_bounded()
        .await;
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Ok(DriverEvent::TurnCompleted { .. })))
    );
    let response = requests(root.path())
        .into_iter()
        .find(|r| r["id"] == "task-fixture" && r.get("method").is_none())
        .unwrap();
    assert_eq!(response["result"]["outcome"]["outcome"], "rejected");
    assert_eq!(
        response["result"]["outcome"]["reason"],
        "Task session is not active"
    );
    driver.shutdown(a).await.unwrap();
    driver.shutdown(b).await.unwrap();
}

#[tokio::test]
async fn context_from_before_the_next_turn_cannot_overwrite_that_turns_live_context() {
    let (root, driver) = fixture_as("aux-context", "kiro");
    let a = driver.create_session(request(root.path())).await.unwrap();
    let read = tokio::spawn({
        let d = driver.clone();
        let id = a.native_session_id.clone();
        async move { d.read_session_usage(&id).await }
    });
    wait_for(|| count(root.path(), "_kiro/session/context") == 1).await;
    let events = driver
        .prompt(&a, prompt(root.path(), "next turn"))
        .await
        .unwrap()
        .finish_bounded()
        .await;
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Ok(DriverEvent::TurnCompleted { .. })))
    );
    driver.shared.quota.observe_kiro_context(
        &a.native_session_id,
        &serde_json::json!({"usagePercentage":22}),
    );
    std::fs::write(root.path().join("context-ready"), b"").unwrap();
    assert!(read.await.unwrap().is_none());
    assert_eq!(
        driver
            .shared
            .quota
            .usage
            .get(Some(&a.native_session_id))
            .unwrap()
            .context_used_percent,
        Some(22.0)
    );
    driver.shutdown(a).await.unwrap();
}

#[tokio::test]
async fn a_context_deadline_does_not_free_native_ownership_or_allow_replay() {
    let (root, driver) = fixture_as("aux-context", "kiro");
    let a = driver.create_session(request(root.path())).await.unwrap();
    let first = tokio::time::timeout(
        Duration::from_secs(4),
        driver.read_session_usage(&a.native_session_id),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(first.context_used_percent.is_none());
    assert_eq!(driver.shared.pending_auxiliary.load(Ordering::Acquire), 1);
    let _ = driver.read_session_usage(&a.native_session_id).await;
    assert_eq!(count(root.path(), "_kiro/session/context"), 1);
    std::fs::write(root.path().join("context-ready"), b"").unwrap();
    wait_for(|| driver.shared.pending_auxiliary.load(Ordering::Acquire) == 0).await;
    assert!(
        driver
            .shared
            .quota
            .usage
            .get(Some(&a.native_session_id))
            .and_then(|u| u.context_used_percent)
            .is_none()
    );
    assert_eq!(
        driver
            .read_session_usage(&a.native_session_id)
            .await
            .unwrap()
            .context_used_percent,
        Some(7.0)
    );
    assert_eq!(count(root.path(), "_kiro/session/context"), 2);
    driver.shutdown(a).await.unwrap();
}
#[tokio::test]
async fn grok_cost_deadline_retains_the_original_read_without_blocking_peer_prompts() {
    let (root, driver) = fixture_as("aux-context", "grok");
    let a = driver.create_session(request(root.path())).await.unwrap();
    let b = driver.create_session(request(root.path())).await.unwrap();
    let events = driver
        .prompt(&a, prompt(root.path(), "grok-cost"))
        .await
        .unwrap()
        .finish_bounded()
        .await;
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Ok(DriverEvent::TurnCompleted { .. })))
    );
    wait_for(|| count(root.path(), "_x.ai/session/usage") == 1).await;
    let original_a = driver.shared.quota.usage.get(Some(&a.native_session_id));
    let original_b = driver.shared.quota.usage.get(Some(&b.native_session_id));
    tokio::time::sleep(Duration::from_millis(2100)).await;
    assert_eq!(driver.shared.pending_auxiliary.load(Ordering::Acquire), 1);
    let events = driver
        .prompt(&b, prompt(root.path(), "peer"))
        .await
        .unwrap()
        .finish_bounded()
        .await;
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Ok(DriverEvent::TurnCompleted { .. })))
    );
    std::fs::write(root.path().join("cost-ready"), b"").unwrap();
    wait_for(|| driver.shared.pending_auxiliary.load(Ordering::Acquire) == 0).await;
    assert_eq!(
        driver.shared.quota.usage.get(Some(&a.native_session_id)),
        original_a
    );
    assert_eq!(
        driver.shared.quota.usage.get(Some(&b.native_session_id)),
        original_b
    );
    assert_eq!(count(root.path(), "_x.ai/session/usage"), 1);
    driver.shutdown(a).await.unwrap();
    driver.shutdown(b).await.unwrap();
}
#[tokio::test]
async fn repeated_local_retirement_cannot_accumulate_unbounded_unanswered_native_reads() {
    let (root, driver) = fixture_as("aux-context", "kiro");
    let mut a = driver.create_session(request(root.path())).await.unwrap();
    let b = driver.create_session(request(root.path())).await.unwrap();
    for attempt in 0..10usize {
        let read = tokio::spawn({
            let d = driver.clone();
            let id = a.native_session_id.clone();
            async move { d.refresh_kiro_context(&id).await }
        });
        if attempt < 8 {
            wait_for(|| count(root.path(), "_kiro/session/context") == attempt + 1).await;
            read.abort();
            let _ = read.await;
        } else {
            read.await.unwrap();
        }
        driver.shutdown(a.clone()).await.unwrap();
        a = driver
            .resume_session(&a.native_session_id, &root.path().to_string_lossy())
            .await
            .unwrap();
    }
    assert_eq!(count(root.path(), "_kiro/session/context"), 8);
    assert_eq!(driver.shared.pending_auxiliary.load(Ordering::Acquire), 8);
    driver.shutdown(a).await.unwrap();
    driver.shutdown(b).await.unwrap();
    wait_for(|| driver.shared.pending_auxiliary.load(Ordering::Acquire) == 0).await;
    assert!(ids(root.path()).iter().all(|id| !present(*id)));
}
