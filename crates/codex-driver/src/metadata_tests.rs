//! Actual private stdio processes only; no installed Codex/account is queried.
use super::*;

fn fixture(plan: &[&str]) -> (tempfile::TempDir, Arc<CodexDriver>) {
    let assets = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    // Same filesystem for hard-linking the immutable fixture executable. No
    // writable executable can leak through another parallel test's fork.
    let root = tempfile::tempdir_in(&assets).unwrap();
    let executable = root.path().join("runtime");
    std::fs::hard_link(assets.join("catalog.py"), &executable).unwrap();
    std::fs::write(
        root.path().join("plan.json"),
        serde_json::to_vec(plan).unwrap(),
    )
    .unwrap();
    let driver =
        Arc::new(CodexDriver::new(&executable, "fixture".into(), "fixture".into()).unwrap());
    (root, driver)
}
fn rows(root: &Path, file: &str) -> Vec<Value> {
    std::fs::read_to_string(root.join(file))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}
async fn wait_for(mut ready: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(12), async {
        while !ready() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}
async fn started(root: &Path, ordinal: usize) -> Value {
    wait_for(|| rows(root, "starts.jsonl").len() > ordinal).await;
    rows(root, "starts.jsonl")[ordinal].clone()
}
fn pids(row: &Value) -> Vec<u32> {
    row["pids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|pid| u32::try_from(pid.as_u64().unwrap()).unwrap())
        .collect()
}
fn present(pid: u32) -> bool {
    let output = std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "pid="])
        .output()
        .unwrap();
    String::from_utf8(output.stdout).unwrap().trim() == pid.to_string()
}
fn count(root: &Path, ordinal: usize, method: &str) -> usize {
    rows(root, "requests.jsonl")
        .iter()
        .filter(|row| row["ordinal"] == ordinal && row["request"]["method"] == method)
        .count()
}
fn verify(catalog: &driver_api::ModelCatalog) {
    assert_eq!(catalog.default_model.as_deref(), Some("fixture-default"));
    assert_eq!(catalog.models.len(), 2);
    let model = catalog
        .models
        .iter()
        .find(|row| row.id == "fixture-default")
        .unwrap();
    assert_eq!(model.default_thinking.as_deref(), Some("medium"));
    assert_eq!(
        model
            .thinking_options
            .iter()
            .map(|v| v.id.as_str())
            .collect::<Vec<_>>(),
        ["medium", "high"]
    );
}
#[tokio::test]
async fn concurrent_catalogs_wait_for_the_original_tree_before_starting_a_replacement() {
    let (root, driver) = fixture(&["blocked-list", "normal"]);
    let first_driver = driver.clone();
    let first = tokio::spawn(async move { first_driver.model_catalog().await });
    let original = started(root.path(), 0).await;
    wait_for(|| count(root.path(), 0, "model/list") == 1).await;
    let mut second = Box::pin(driver.model_catalog());
    assert!(
        tokio::time::timeout(Duration::from_secs(4), &mut second)
            .await
            .is_err()
    );
    assert_eq!(
        rows(root.path(), "starts.jsonl").len(),
        1,
        "parallel catalog helper started"
    );
    first.abort();
    let _ = first.await;
    verify(
        &tokio::time::timeout(Duration::from_secs(10), second)
            .await
            .unwrap()
            .unwrap(),
    );
    let replacement = started(root.path(), 1).await;
    assert_eq!(replacement["previousAlive"], json!([]));
    assert!(pids(&original).iter().all(|pid| !present(*pid)));
    assert!(pids(&replacement).iter().all(|pid| !present(*pid)));
    assert_eq!(count(root.path(), 1, "model/list"), 2);
    assert_eq!(count(root.path(), 1, "thread/start"), 0);
}
#[tokio::test]
async fn cancelling_during_initialize_retires_the_scope_without_sending_a_catalog_request() {
    let (root, driver) = fixture(&["blocked-initialize", "normal"]);
    let first_driver = driver.clone();
    let first = tokio::spawn(async move { first_driver.model_catalog().await });
    let original = started(root.path(), 0).await;
    wait_for(|| count(root.path(), 0, "initialize") == 1).await;
    first.abort();
    let _ = first.await;
    verify(
        &tokio::time::timeout(Duration::from_secs(10), driver.model_catalog())
            .await
            .unwrap()
            .unwrap(),
    );
    let next = started(root.path(), 1).await;
    assert_eq!(next["previousAlive"], json!([]));
    assert_eq!(count(root.path(), 0, "model/list"), 0);
    assert!(pids(&original).iter().all(|pid| !present(*pid)));
    assert!(pids(&next).iter().all(|pid| !present(*pid)));
}
#[tokio::test]
async fn a_catalog_failure_is_cleaned_before_return_and_does_not_block_the_next_query() {
    let (root, driver) = fixture(&["error", "normal"]);
    assert!(driver.model_catalog().await.is_err());
    let original = started(root.path(), 0).await;
    assert!(pids(&original).iter().all(|pid| !present(*pid)));
    verify(&driver.model_catalog().await.unwrap());
    let next = started(root.path(), 1).await;
    assert_eq!(next["previousAlive"], json!([]));
    assert!(pids(&next).iter().all(|pid| !present(*pid)));
}

#[tokio::test]
async fn a_cancelled_queued_catalog_never_launches_later() {
    let (root, driver) = fixture(&["normal"]);
    let admission = driver.catalog_probe.clone().lock_owned().await;
    assert!(
        tokio::time::timeout(Duration::from_millis(50), driver.model_catalog())
            .await
            .is_err()
    );
    assert!(rows(root.path(), "starts.jsonl").is_empty());
    drop(admission);
    verify(&driver.model_catalog().await.unwrap());
    assert_eq!(rows(root.path(), "starts.jsonl").len(), 1);
    assert!(
        pids(&started(root.path(), 0).await)
            .iter()
            .all(|pid| !present(*pid))
    );
}

#[tokio::test]
async fn reading_catalogs_and_child_history_never_stops_an_active_peer_turn() {
    let (root, driver) = fixture(&["peer", "normal", "normal"]);
    let request = SessionRequest {
        working_directory: root.path().to_string_lossy().into_owned(),
        additional_directories: vec![],
        app_mcps: vec![],
        permission_mode: None,
    };
    let peer = driver.create_session(request).await.unwrap();
    let peer_ids = pids(&started(root.path(), 0).await);
    let _running = driver
        .prompt(
            &peer,
            PromptRequest {
                working_directory: root.path().to_string_lossy().into_owned(),
                additional_directories: vec![],
                attachments: vec![],
                permission_mode: None,
                context: vec![],
                prompt: "slow".into(),
            },
        )
        .await
        .unwrap();
    wait_for(|| count(root.path(), 0, "turn/start") == 1).await;
    let native = driver.sessions.lock().await[&peer.native_session_id]
        .native
        .clone()
        .unwrap();
    verify(&driver.model_catalog().await.unwrap());
    let history = driver.read_subagent("child-of-peer").await.unwrap();
    assert!(history.contains(&json!({"type":"message","text":"private child answer"})));
    assert!(
        pids(&started(root.path(), 2).await)
            .iter()
            .all(|pid| !present(*pid))
    );
    assert_eq!(count(root.path(), 2, "thread/start"), 0);
    assert_eq!(count(root.path(), 2, "turn/start"), 0);
    assert!(peer_ids.iter().all(|pid| present(*pid)));
    assert!(!native.stopping());
    assert_eq!(count(root.path(), 0, "turn/start"), 1);
    assert_eq!(count(root.path(), 1, "thread/start"), 0);
    assert_eq!(count(root.path(), 1, "turn/start"), 0);
    assert!(
        pids(&started(root.path(), 1).await)
            .iter()
            .all(|pid| !present(*pid))
    );
    driver.shutdown(peer).await.unwrap();
}

#[tokio::test]
async fn child_history_waits_for_original_cleanup_before_starting_another_helper() {
    let (root, driver) = fixture(&["blocked-read", "normal"]);
    let first_driver = driver.clone();
    let first = tokio::spawn(async move { first_driver.read_subagent("original-child").await });
    let original = started(root.path(), 0).await;
    wait_for(|| count(root.path(), 0, "thread/read") == 1).await;
    let mut second = Box::pin(driver.read_subagent("next-child"));
    let observed = tokio::time::timeout(Duration::from_secs(4), &mut second).await;
    let started_before_cancel = rows(root.path(), "starts.jsonl").len();
    first.abort();
    let _ = first.await;
    let history = match observed {
        Ok(result) => result.unwrap(),
        Err(_) => tokio::time::timeout(Duration::from_secs(10), second)
            .await
            .unwrap()
            .unwrap(),
    };
    let replacement = started(root.path(), 1).await;
    // Clean the private original before asserting even with the unfixed code.
    wait_for(|| pids(&original).iter().all(|pid| !present(*pid))).await;
    wait_for(|| pids(&replacement).iter().all(|pid| !present(*pid))).await;
    assert_eq!(
        started_before_cancel, 1,
        "parallel history helper started before original cleanup"
    );
    assert_eq!(replacement["previousAlive"], json!([]));
    assert!(history.contains(&json!({"type":"message","text":"private child answer"})));
    assert_eq!(
        history.last(),
        Some(&json!({"type":"status","status":"completed"}))
    );
    assert_eq!(count(root.path(), 1, "thread/start"), 0);
    assert_eq!(count(root.path(), 1, "turn/start"), 0);
    let request = rows(root.path(), "requests.jsonl")
        .into_iter()
        .find(|row| row["ordinal"] == 1 && row["request"]["method"] == "thread/read")
        .unwrap();
    assert_eq!(
        request["request"]["params"],
        json!({"threadId":"next-child","includeTurns":true})
    );
}

#[tokio::test]
async fn cancelling_history_during_initialize_never_reads_or_replays_the_child() {
    let (root, driver) = fixture(&["blocked-initialize", "normal"]);
    let first_driver = driver.clone();
    let first = tokio::spawn(async move { first_driver.read_subagent("cancelled-child").await });
    let original = started(root.path(), 0).await;
    wait_for(|| count(root.path(), 0, "initialize") == 1).await;
    first.abort();
    let _ = first.await;
    let history =
        tokio::time::timeout(Duration::from_secs(10), driver.read_subagent("other-child"))
            .await
            .unwrap()
            .unwrap();
    assert!(history.contains(&json!({"type":"message","text":"private child answer"})));
    let next = started(root.path(), 1).await;
    assert_eq!(next["previousAlive"], json!([]));
    assert_eq!(count(root.path(), 0, "thread/read"), 0);
    assert!(pids(&original).iter().all(|pid| !present(*pid)));
    assert!(pids(&next).iter().all(|pid| !present(*pid)));
}

#[tokio::test]
async fn rejected_history_returns_only_after_original_cleanup_and_allows_a_new_read() {
    let (root, driver) = fixture(&["error", "normal"]);
    assert!(driver.read_subagent("rejected-child").await.is_err());
    let original = started(root.path(), 0).await;
    assert!(pids(&original).iter().all(|pid| !present(*pid)));
    assert_eq!(count(root.path(), 0, "thread/read"), 1);
    let history = driver.read_subagent("other-child").await.unwrap();
    assert_eq!(
        history.last(),
        Some(&json!({"type":"status","status":"completed"}))
    );
    let next = started(root.path(), 1).await;
    assert_eq!(next["previousAlive"], json!([]));
    assert!(pids(&next).iter().all(|pid| !present(*pid)));
}

#[tokio::test]
async fn a_cancelled_queued_history_never_launches_or_blocks_catalogs() {
    let (root, driver) = fixture(&["normal", "normal"]);
    let admission = driver.history_probe.clone().lock_owned().await;
    assert!(
        tokio::time::timeout(
            Duration::from_millis(50),
            driver.read_subagent("cancelled-child")
        )
        .await
        .is_err()
    );
    assert!(rows(root.path(), "starts.jsonl").is_empty());
    verify(&driver.model_catalog().await.unwrap());
    assert_eq!(rows(root.path(), "starts.jsonl").len(), 1);
    drop(admission);
    driver.read_subagent("other-child").await.unwrap();
    assert_eq!(rows(root.path(), "starts.jsonl").len(), 2);
    assert_eq!(count(root.path(), 0, "thread/read"), 0);
    assert_eq!(count(root.path(), 1, "thread/read"), 1);
    for row in rows(root.path(), "starts.jsonl") {
        assert!(pids(&row).iter().all(|pid| !present(*pid)));
    }
}
