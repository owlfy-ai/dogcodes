//! Host boundary: resolve workspace roots from Task IDs, never client paths.
pub mod batch_file_actions;
pub mod git_credentials;
pub mod git_operations;
mod git_push_target;
pub mod git_workflow;
pub mod mutation_operations;
pub mod operation_history;
mod operation_journal;
mod project_analysis;
mod recorded_changes;
mod remote_actions;
pub mod service;
mod snapshot_leases;
pub mod watch;
pub use snapshot_leases::LeaseRequest;
use snapshot_leases::SnapshotLeases;
pub mod workspaces;

use agent_domain::{ArtifactId, TaskId};
pub use agent_workspace::review::git::{BranchSummary, CommitSummary};
use agent_workspace::review::text::{TextBlock, TextComparison};
use agent_workspace::review::{
    CompareMode, FileChange, ReviewStore,
    git::{GitReview, ReviewRoot},
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileStatistics {
    pub additions: Option<usize>,
    pub deletions: Option<usize>,
    pub binary: bool,
    pub error: Option<String>,
}

#[derive(Clone)]
struct TextPlan {
    blocks: Vec<TextBlock>,
    additions: usize,
    deletions: usize,
}

fn read_text(store: &ReviewStore, change: &FileChange) -> Result<(String, String), String> {
    let read = |version: &Option<agent_workspace::review::FileVersion>| -> Result<String, String> {
        version
            .as_ref()
            .map(|v| {
                String::from_utf8(store.read_blob(&v.hash)?)
                    .map_err(|_| "backendText.m8d0294c0".into())
            })
            .transpose()
            .map(Option::unwrap_or_default)
    };
    Ok((read(&change.before)?, read(&change.after)?))
}

fn prepare_statistics(root: &mut RootChanges, store: &ReviewStore) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    for change in &root.files {
        let binary = change.before.iter().chain(&change.after).any(|v| v.binary);
        let mut stats = FileStatistics {
            additions: None,
            deletions: None,
            binary,
            error: None,
        };
        if !binary {
            let result = if std::time::Instant::now() >= deadline {
                Err("backendText.me2f56d98".into())
            } else {
                read_text(store, change).and_then(|(before, after)| {
                    if root.root.git {
                        agent_workspace::review::text::compare_git(&before, &after)
                    } else {
                        agent_workspace::review::text::compare(&before, &after)
                    }
                })
            };
            match result {
                Ok(text) => {
                    stats.additions = Some(text.additions);
                    stats.deletions = Some(text.deletions);
                    root.text_plans.insert(
                        change.path.clone(),
                        TextPlan {
                            blocks: text.blocks,
                            additions: text.additions,
                            deletions: text.deletions,
                        },
                    );
                }
                Err(error) => stats.error = Some(error),
            }
        }
        root.statistics.insert(change.path.clone(), stats);
    }
}

pub struct ReviewHost {
    pub git_operations: Arc<git_operations::GitOperations>,
    pub mutations: Arc<mutation_operations::MutationOperations>,
    pub changes: watch::Changes,
    pub directory: PathBuf,
    pub gate: tokio::sync::Mutex<()>,
    pub prompts: git_credentials::GitPrompts,
    snapshots: std::sync::Mutex<BTreeMap<String, ReviewSnapshot>>,
    leases: std::sync::Mutex<SnapshotLeases>,
    owner: std::sync::Mutex<Option<agent_workspace::review::storage::SnapshotOwner>>,
}

impl ReviewHost {
    #[must_use]
    pub fn new(directory: PathBuf) -> Self {
        let changes = watch::Changes::new();
        Self {
            git_operations: Arc::new(
                git_operations::GitOperations::new(directory.join("git-operations"))
                    .with_changes(changes.source()),
            ),
            mutations: Arc::new(
                mutation_operations::MutationOperations::new(directory.join("file-operations"))
                    .with_changes(changes.source()),
            ),
            changes,
            directory,
            gate: tokio::sync::Mutex::new(()),
            prompts: git_credentials::GitPrompts::default(),
            snapshots: std::sync::Mutex::new(BTreeMap::new()),
            leases: std::sync::Mutex::new(SnapshotLeases::default()),
            owner: std::sync::Mutex::new(None),
        }
    }

    #[doc(hidden)]
    pub fn has_snapshot(&self, id: &str) -> bool {
        self.snapshots.lock().unwrap().contains_key(id)
    }

    pub fn forget_task(&self, task: &str) -> Result<(), String> {
        self.git_operations.forget_task(task)?;
        self.mutations.forget_task(task)?;
        self.changes.changed();
        let mut cache = self
            .snapshots
            .lock()
            .map_err(|_| "Review cache unavailable")?;
        let mut leases = self.leases.lock().map_err(|_| "Review lease unavailable")?;
        cache.retain(|id, snapshot| {
            if snapshot.task_id == task {
                leases.remove(id);
                false
            } else {
                true
            }
        });
        Ok(())
    }

    pub fn file_change(
        &self,
        task_id: &str,
        snapshot_id: &str,
        root_id: &str,
        path: &str,
    ) -> Result<FileChange, String> {
        let cache = self
            .snapshots
            .lock()
            .map_err(|_| "Review cache unavailable")?;
        let snapshot = cache
            .get(snapshot_id)
            .filter(|s| s.task_id == task_id)
            .ok_or("backendText.md9b1193f")?;
        snapshot
            .roots
            .iter()
            .find(|r| r.root.id == root_id)
            .and_then(|r| r.files.iter().find(|f| f.path == path))
            .cloned()
            .ok_or_else(|| "backendText.m978c6287".into())
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RootChanges {
    pub root: ReviewRoot,
    pub mode: CompareMode,
    pub base: String,
    pub target: String,
    pub files: Vec<FileChange>,
    pub untracked: Vec<String>,
    pub excluded_roots: Vec<String>,
    pub error: Option<String>,
    pub statistics: BTreeMap<String, FileStatistics>,
    #[serde(skip)]
    text_plans: BTreeMap<String, TextPlan>,
    #[serde(skip)]
    git_comparison: Option<agent_workspace::review::git::GitComparison>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewSnapshot {
    pub id: String,
    pub task_id: String,
    pub roots: Vec<RootChanges>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewQuery {
    pub task_id: String,
    pub root_id: Option<String>,
    pub mode: CompareMode,
    pub reference: Option<String>,
    pub parent: Option<usize>,
    #[serde(default)]
    pub probe: bool,
}

async fn query_root(
    root: ReviewRoot,
    query: ReviewQuery,
    directory: PathBuf,
    recorded: Arc<recorded_changes::RecordedChanges>,
) -> Result<RootChanges, String> {
    let store = ReviewStore::open(directory.clone())?;
    let service = GitReview::default();
    let mode = if root.git {
        query.mode.clone()
    } else {
        CompareMode::Session
    };
    let mut item = RootChanges {
        root: root.clone(),
        mode: mode.clone(),
        base: String::new(),
        target: String::new(),
        files: vec![],
        untracked: vec![],
        excluded_roots: vec![],
        error: None,
        statistics: BTreeMap::new(),
        text_plans: BTreeMap::new(),
        git_comparison: None,
    };
    if root.git {
        match service
            .compare(
                &root,
                mode,
                query.reference.as_deref(),
                query.parent.unwrap_or(0),
                &store,
            )
            .await
        {
            Ok(result) => {
                item.git_comparison = Some(result.clone());
                item.files = result.changes;
                item.base = result.base;
                item.target = result.target;
                item.untracked = result.untracked;
            }
            Err(error) => item.error = Some(error),
        }
    } else {
        let data = directory.clone();
        let path = PathBuf::from(&root.path);
        let before = recorded.get(&root.id).cloned().unwrap_or_default();
        let result = tokio::task::spawn_blocking(move || {
            let store = ReviewStore::open(data)?;
            store.recorded_file_changes(&path, &before)
        })
        .await
        .map_err(|_| "Recorded changes query failed")?;
        match result {
            Ok(files) => item.files = files,
            Err(error) => item.error = Some(error),
        }
        item.base = "first-recorded-change".into();
        item.target = "working".into();
    }
    if query.probe {
        return Ok(item);
    }
    let directory = directory.clone();
    let item = tokio::task::spawn_blocking(move || {
        let store = ReviewStore::open(directory)?;
        prepare_statistics(&mut item, &store);
        Ok::<_, String>(item)
    })
    .await
    .map_err(|_| "Review statistics task failed")??;
    Ok(item)
}

async fn query_roots(
    selected: &[ReviewRoot],
    query: &ReviewQuery,
    directory: &std::path::Path,
    recorded: Arc<recorded_changes::RecordedChanges>,
) -> Result<Vec<RootChanges>, String> {
    // Bound disk/process pressure while independent roots are read concurrently.
    // Keep the operation gate and storage guard until every worker has finished.
    let mut output = Vec::new();
    for chunk in selected.chunks(3) {
        let mut workers = tokio::task::JoinSet::new();
        for (index, root) in chunk.iter().cloned().enumerate() {
            let query = query.clone();
            let directory = directory.to_path_buf();
            let recorded = recorded.clone();
            workers
                .spawn(async move { (index, query_root(root, query, directory, recorded).await) });
        }
        let mut completed = Vec::new();
        let mut failure = None;
        while let Some(result) = workers.join_next().await {
            match result {
                Ok((index, Ok(item))) => completed.push((index, item)),
                Ok((_, Err(error))) => failure = Some(error),
                Err(_) => failure = Some("Review query task failed".into()),
            }
        }
        if let Some(error) = failure {
            return Err(error);
        }
        completed.sort_by_key(|(index, _)| *index);
        output.extend(completed.into_iter().map(|(_, item)| item));
    }
    Ok(output)
}

fn select_query_roots(
    all: Vec<ReviewRoot>,
    query: &ReviewQuery,
) -> Result<Vec<ReviewRoot>, String> {
    if query.root_id.is_none() && matches!(query.mode, CompareMode::Committed | CompareMode::Branch)
    {
        return Err("backendText.m5d8e854b".into());
    }
    let selected: Vec<_> = all
        .into_iter()
        .filter(|r| query.root_id.as_ref().is_none_or(|id| id == &r.id))
        .collect();
    if selected.is_empty() {
        return Err("backendText.m3149b9e1".into());
    }
    let git_only = matches!(
        query.mode,
        CompareMode::Unstaged | CompareMode::Staged | CompareMode::Committed | CompareMode::Branch
    );
    if query.root_id.is_some() && git_only && selected.iter().any(|r| !r.git) {
        return Err("backendText.m8a34f77b".into());
    }
    if query.mode == CompareMode::Session && selected.iter().any(|r| r.git) {
        return Err("backendText.m976b4fb6".into());
    }
    let selected: Vec<_> = selected
        .into_iter()
        .filter(|r| !git_only || r.git)
        .collect();
    if selected.is_empty() {
        return Err("backendText.m8f247f82".into());
    }
    Ok(selected)
}

fn publish_snapshot(
    cache: &mut BTreeMap<String, ReviewSnapshot>,
    leases: &mut SnapshotLeases,
    snapshot: ReviewSnapshot,
    now: std::time::Instant,
) -> Result<Option<String>, String> {
    leases.retain(|id| cache.contains_key(id));
    let evicted = if cache.len() >= 16 {
        let victim = cache
            .keys()
            .find(|id| !leases.protected(id, now))
            .cloned()
            .ok_or("backendText.md6f5edbd")?;
        cache.remove(&victim);
        leases.remove(&victim);
        Some(victim)
    } else {
        None
    };
    leases.publish(&snapshot.id, now);
    cache.insert(snapshot.id.clone(), snapshot);
    Ok(evicted)
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewFile {
    pub text: Option<agent_workspace::review::text::TextComparison>,
    pub binary: bool,
    pub before_image: Option<String>,
    pub after_image: Option<String>,
}

fn image_preview(
    store: &ReviewStore,
    version: Option<&agent_workspace::review::FileVersion>,
    path: &str,
) -> Result<Option<String>, String> {
    let Some(version) = version else {
        return Ok(None);
    };
    if version.bytes > 20 * 1024 * 1024 {
        return Ok(None);
    }
    let bytes = store.read_blob(&version.hash)?;
    Ok(image_data_url(&bytes, path))
}

#[must_use]
pub fn image_data_url(bytes: &[u8], path: &str) -> Option<String> {
    use base64::Engine;
    // SVG stays in an img resource: never inject its markup into the webview.
    let mime = if path.to_ascii_lowercase().ends_with(".svg") {
        "image/svg+xml"
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        "image/png"
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        "image/jpeg"
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        "image/gif"
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        "image/webp"
    } else if bytes.starts_with(b"BM") {
        "image/bmp"
    } else if bytes.starts_with(&[0, 0, 1, 0]) {
        "image/x-icon"
    } else {
        return None;
    };
    Some(format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexAction {
    Stage,
    Unstage,
}

// Tauri maps each frontend argument to its own parameter.

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscardRequest {
    pub task_id: String,
    pub snapshot_id: String,
    pub root_id: String,
    pub path: String,
    pub confirmed: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_workspace::review::ChangeKind;
    use agent_workspace::review::{SnapshotBudget, compare_snapshots};
    use std::fmt::Write;

    #[tokio::test]
    async fn directory_scope_filters_git_modes_and_freezes_each_repository() {
        let data = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let mut all = Vec::new();
        for i in 0..3 {
            let path = workspace.path().join(i.to_string());
            std::fs::create_dir(&path).unwrap();
            let path = path.canonicalize().unwrap();
            std::fs::write(path.join("same.txt"), "base\n").unwrap();
            if i < 2 {
                let git = |args: &[&str]| {
                    let result = std::process::Command::new("git")
                        .current_dir(&path)
                        .args(args)
                        .output()
                        .unwrap();
                    assert!(
                        result.status.success(),
                        "{}",
                        String::from_utf8_lossy(&result.stderr)
                    );
                };
                git(&["init", "-q"]);
                git(&["add", "same.txt"]);
                git(&[
                    "-c",
                    "user.name=Review Test",
                    "-c",
                    "user.email=review@example.invalid",
                    "-c",
                    "commit.gpgsign=false",
                    "commit",
                    "-qm",
                    "baseline",
                ]);
                std::fs::write(path.join("same.txt"), format!("staged {i}\n")).unwrap();
                git(&["add", "same.txt"]);
                std::fs::write(path.join("same.txt"), format!("working {i}\n")).unwrap();
            }
            all.push(ReviewRoot {
                id: i.to_string(),
                path: path.to_str().unwrap().into(),
                git: i < 2,
                scopes: vec![String::new()],
            });
        }
        let mut query = ReviewQuery {
            task_id: "task".into(),
            root_id: None,
            mode: CompareMode::Uncommitted,
            reference: None,
            parent: None,
            probe: false,
        };
        assert_eq!(select_query_roots(all.clone(), &query).unwrap().len(), 3);
        let store = ReviewStore::open(data.path().to_path_buf()).unwrap();
        for mode in [CompareMode::Staged, CompareMode::Unstaged] {
            query.mode = mode.clone();
            let selected = select_query_roots(all.clone(), &query).unwrap();
            assert_eq!(selected.len(), 2);
            let output = query_roots(&selected, &query, data.path(), Arc::default())
                .await
                .unwrap();
            for (i, item) in output.iter().enumerate() {
                assert_eq!(item.root.id, i.to_string());
                assert!(item.error.is_none(), "{:?}", item.error);
                assert_eq!(item.files.len(), 1);
                let expected = if mode == CompareMode::Staged {
                    ("base\n".into(), format!("staged {i}\n"))
                } else {
                    (format!("staged {i}\n"), format!("working {i}\n"))
                };
                assert_eq!(read_text(&store, &item.files[0]).unwrap(), expected);
            }
            query.root_id = Some("2".into());
            assert!(select_query_roots(all.clone(), &query).is_err());
            query.root_id = Some("1".into());
            assert_eq!(select_query_roots(all.clone(), &query).unwrap()[0].id, "1");
            query.root_id = None;
        }
        for mode in [CompareMode::Committed, CompareMode::Branch] {
            query.mode = mode;
            assert!(select_query_roots(all.clone(), &query).is_err());
            query.root_id = Some("1".into());
            assert_eq!(select_query_roots(all.clone(), &query).unwrap().len(), 1);
            query.root_id = None;
        }
        query.mode = CompareMode::Uncommitted;
        query.root_id = Some("2".into());
        assert!(!select_query_roots(all.clone(), &query).unwrap()[0].git);
        query.root_id = Some("missing".into());
        assert!(select_query_roots(all, &query).is_err());
    }

    #[tokio::test]
    async fn concurrent_roots_keep_order_recorded_changes_and_individual_errors() {
        let data = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let store = ReviewStore::open(data.path().to_path_buf()).unwrap();
        let mut roots = Vec::new();
        let mut recorded = recorded_changes::RecordedChanges::new();
        for i in 0..4 {
            let path = workspace.path().join(i.to_string());
            std::fs::create_dir(&path).unwrap();
            std::fs::write(path.join("same.txt"), format!("before {i}\n")).unwrap();
            let before = format!("before {i}\n");
            let hex = before
                .as_bytes()
                .iter()
                .fold(String::new(), |mut hex, byte| {
                    write!(hex, "{byte:02x}").unwrap();
                    hex
                });
            let version = serde_json::json!({"hex":hex,"sha256":store.put_blob(before.as_bytes()).unwrap(),"mode":"100644"});
            recorded.entry(i.to_string()).or_default().insert(
                "same.txt".into(),
                if i == 1 {
                    serde_json::json!({"contentOmitted":true})
                } else {
                    version
                },
            );
            std::fs::write(path.join("same.txt"), format!("after {i}\n")).unwrap();
            roots.push(ReviewRoot {
                id: i.to_string(),
                path: path.to_str().unwrap().into(),
                git: false,
                scopes: vec![String::new()],
            });
        }
        let query = ReviewQuery {
            task_id: "task".into(),
            root_id: None,
            mode: CompareMode::Uncommitted,
            reference: None,
            parent: None,
            probe: false,
        };
        let result = query_roots(&roots, &query, data.path(), Arc::new(recorded))
            .await
            .unwrap();
        for (i, item) in result.iter().enumerate() {
            assert_eq!(item.root.id, i.to_string());
            if i == 1 {
                assert!(item.error.is_some());
                continue;
            }
            assert!(item.error.is_none(), "root {i}: {:?}", item.error);
            assert_eq!(item.files.len(), 1);
            assert_eq!(
                read_text(&store, &item.files[0]).unwrap(),
                (format!("before {i}\n"), format!("after {i}\n"))
            );
            assert_eq!(item.statistics["same.txt"].additions, Some(1));
        }
    }

    #[test]
    fn snapshot_eviction_preserves_active_leases_and_expires_abandoned_readers() {
        let now = std::time::Instant::now();
        let mut cache = BTreeMap::new();
        let mut leases = SnapshotLeases::default();
        let snapshot = |id: &str| ReviewSnapshot {
            id: id.into(),
            task_id: "task".into(),
            roots: vec![],
        };
        for index in 0..16 {
            publish_snapshot(
                &mut cache,
                &mut leases,
                snapshot(&format!("{index:02}")),
                now,
            )
            .unwrap();
        }
        assert!(publish_snapshot(&mut cache, &mut leases, snapshot("new"), now).is_err());
        assert_eq!(cache.len(), 16);
        let first = uuid::Uuid::new_v4().to_string();
        let second = uuid::Uuid::new_v4().to_string();
        let reader = |id: &str, revision, release| LeaseRequest {
            task_id: "task".into(),
            snapshot_id: "07".into(),
            lease_id: id.into(),
            revision,
            release,
        };
        leases.update(&reader(&first, 1, false), now).unwrap();
        leases.update(&reader(&second, 1, false), now).unwrap();
        leases.update(&reader(&first, 2, true), now).unwrap();
        assert!(publish_snapshot(&mut cache, &mut leases, snapshot("new"), now).is_err());
        assert!(cache.contains_key("07"));
        leases.update(&reader(&second, 2, true), now).unwrap();
        publish_snapshot(&mut cache, &mut leases, snapshot("new"), now).unwrap();
        assert!(cache.contains_key("00"));
        assert!(!cache.contains_key("07"));
        publish_snapshot(
            &mut cache,
            &mut leases,
            snapshot("later"),
            now + std::time::Duration::from_secs(121),
        )
        .unwrap();
        assert_eq!(cache.len(), 16);
        assert!(!cache.contains_key("00"));
        assert_eq!(
            cache
                .keys()
                .filter(|id| leases.protected(id, now + std::time::Duration::from_secs(121)))
                .count(),
            1
        );
    }

    #[test]
    fn statistics_and_plan_use_frozen_blobs_not_new_disk_contents() {
        let data = tempfile::tempdir().unwrap();
        let files = tempfile::tempdir().unwrap();
        let store = ReviewStore::open(data.path().into()).unwrap();
        std::fs::write(files.path().join("a"), "old\n").unwrap();
        let before = store.scan(files.path(), SnapshotBudget::default()).unwrap();
        std::fs::write(files.path().join("a"), "new\nextra\n").unwrap();
        let after = store.scan(files.path(), SnapshotBudget::default()).unwrap();
        let changes = compare_snapshots(&before, &after).unwrap();
        std::fs::write(files.path().join("a"), "later content").unwrap();
        let mut root = RootChanges {
            root: ReviewRoot {
                id: "root".into(),
                path: files.path().to_string_lossy().into(),
                git: false,
                scopes: vec![],
            },
            mode: CompareMode::Session,
            base: "session-start".into(),
            target: "working".into(),
            files: changes,
            untracked: vec![],
            excluded_roots: vec![],
            error: None,
            statistics: BTreeMap::new(),
            text_plans: BTreeMap::new(),
            git_comparison: None,
        };
        prepare_statistics(&mut root, &store);
        let stats = &root.statistics["a"];
        let plan = &root.text_plans["a"];
        assert_eq!((stats.additions, stats.deletions), (Some(2), Some(1)));
        assert_eq!((plan.additions, plan.deletions), (2, 1));
        assert_eq!(
            read_text(&store, &root.files[0]).unwrap(),
            ("old\n".into(), "new\nextra\n".into())
        );
        assert!(
            !serde_json::to_value(&root)
                .unwrap()
                .as_object()
                .unwrap()
                .contains_key("textPlans")
        );
    }

    #[test]
    fn file_reads_require_exact_snapshot_task_root_and_path() {
        let host = ReviewHost::new(PathBuf::new());
        host.snapshots.lock().unwrap().insert(
            "snapshot".into(),
            ReviewSnapshot {
                id: "snapshot".into(),
                task_id: "task".into(),
                roots: vec![RootChanges {
                    root: ReviewRoot {
                        id: "root".into(),
                        path: "/not-read".into(),
                        git: true,
                        scopes: vec![],
                    },
                    mode: CompareMode::Uncommitted,
                    base: "HEAD".into(),
                    target: "working".into(),
                    files: vec![FileChange {
                        path: "file.txt".into(),
                        previous_path: None,
                        kind: ChangeKind::Added,
                        before: None,
                        after: None,
                    }],
                    untracked: vec![],
                    excluded_roots: vec![],
                    error: None,
                    statistics: BTreeMap::new(),
                    text_plans: BTreeMap::new(),
                    git_comparison: None,
                }],
            },
        );
        assert!(
            host.file_change("task", "snapshot", "root", "file.txt")
                .is_ok()
        );
        for (task, snapshot, root, path) in [
            ("other-task", "snapshot", "root", "file.txt"),
            ("task", "expired", "root", "file.txt"),
            ("task", "snapshot", "other-root", "file.txt"),
            ("task", "snapshot", "root", "../file.txt"),
            ("task", "snapshot", "root", "/file.txt"),
        ] {
            assert!(host.file_change(task, snapshot, root, path).is_err());
        }
    }
}

async fn run_commit_command(
    path: &std::path::Path,
    args: &[&str],
) -> Result<std::process::Output, String> {
    run_commit_command_tagged(path, args, None).await
}
async fn run_commit_command_tagged(
    path: &std::path::Path,
    args: &[&str],
    reflog_action: Option<&str>,
) -> Result<std::process::Output, String> {
    let mut command = agent_runtime::background_command(agent_workspace::git_executable());
    command
        .current_dir(path)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    // Held until Git ends. Someone answering needs longer than Git usually takes.
    let askpass = git_credentials::AskpassServer::for_this_operation()
        .await
        .filter(|askpass| askpass.apply(&mut command).is_ok());
    let limit = if askpass.is_some() {
        std::time::Duration::from_mins(10)
    } else {
        std::time::Duration::from_mins(2)
    };
    if let Some(action) = reflog_action {
        command.env("GIT_REFLOG_ACTION", action);
    }
    // Spawn errors prove that no Git process started. After spawning, a timeout
    // or lost output cannot prove that a commit/push/pull had no effect.
    command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let child = command.spawn().map_err(|e| e.to_string())?;
    let output = collect_git_output(child, limit).await;
    drop(askpass);
    output
}
const UNKNOWN_GIT_EFFECT: &str = "backendText.mf64b0f53";
async fn collect_git_output(
    child: tokio::process::Child,
    timeout: std::time::Duration,
) -> Result<std::process::Output, String> {
    tokio::time::timeout(timeout, child.wait_with_output())
        .await
        .map_err(|_| format!("backendText.mabc79b63\u{1e}{}", serde_json::to_string(&[UNKNOWN_GIT_EFFECT.to_string()]).unwrap_or_else(|_| "[]".into())))?
        .map_err(|e| format!("backendText.ma5bce326\u{1e}{}", serde_json::to_string(&[UNKNOWN_GIT_EFFECT.to_string(), e.to_string()]).unwrap_or_else(|_| "[]".into())))
}
fn require_git_success(output: std::process::Output) -> Result<(), String> {
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{}{}{}",
            if output.status.code().is_none() {
                UNKNOWN_GIT_EFFECT
            } else {
                ""
            },
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ))
    }
}
// Return false to ask the user before staging anything.
#[cfg(test)]
async fn commit_changes(
    path: &std::path::Path,
    message: &str,
    stage_all: bool,
) -> Result<bool, String> {
    commit_changes_tagged(path, message, stage_all, None).await
}
async fn commit_changes_tagged(
    path: &std::path::Path,
    message: &str,
    stage_all: bool,
    reflog_action: Option<&str>,
) -> Result<bool, String> {
    let staged = run_commit_command(path, &["diff", "--cached", "--quiet", "--exit-code"]).await?;
    match staged.status.code() {
        Some(0) => {
            if !stage_all {
                return Ok(false);
            }
            require_git_success(run_commit_command(path, &["add", "--all", "--", "."]).await?)?;
        }
        Some(1) => {} // Preserve partial staging, even after a confirmation was opened.
        _ => {
            require_git_success(staged)?;
        }
    }
    require_git_success(
        run_commit_command_tagged(path, &["commit", "-m", message], reflog_action).await?,
    )?;
    Ok(true)
}

// Validate the immutable diff the user reviewed against disk immediately before
// committing. Checking HEAD and the index also catches an external commit whose
// remaining working diff happens to look identical.
async fn validate_commit_preview(
    root: &ReviewRoot,
    expected: &agent_workspace::review::git::GitComparison,
    store: &ReviewStore,
) -> Result<(), String> {
    const STALE: &str = "backendText.m5659c1af";
    if root.path != expected.root.path
        || root.scopes != expected.root.scopes
        || root.git != expected.root.git
        || !matches!(
            expected.mode,
            CompareMode::Uncommitted | CompareMode::Unstaged | CompareMode::Staged
        )
    {
        return Err(STALE.into());
    }
    let current = GitReview::default()
        .compare(root, expected.mode.clone(), None, 0, store)
        .await?;
    if current.version != expected.version
        || current.repository_version != expected.repository_version
    {
        return Err(STALE.into());
    }
    if current.changes.is_empty() {
        return Err("backendText.mec1c88c2".into());
    }
    Ok(())
}

// Tauri maps each frontend argument to its own parameter.

#[cfg(test)]
mod smart_commit_tests {
    use super::*;
    #[tokio::test]
    async fn commit_preview_rejects_external_commits_staging_and_file_changes() {
        let repo = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let path = repo.path().canonicalize().unwrap();
        for args in [
            vec!["init"],
            vec!["config", "user.email", "test@example.com"],
            vec!["config", "user.name", "Test"],
            vec!["config", "commit.gpgsign", "false"],
        ] {
            require_git_success(run_commit_command(&path, &args).await.unwrap()).unwrap();
        }
        std::fs::write(path.join("a"), "initial").unwrap();
        commit_changes(&path, "initial", true).await.unwrap();
        let root = ReviewRoot {
            id: "repo".into(),
            path: path.to_str().unwrap().into(),
            git: true,
            scopes: vec![String::new()],
        };
        let store = ReviewStore::open(cache.path().into()).unwrap();
        let git = GitReview::default();
        let clean = git
            .compare(&root, CompareMode::Uncommitted, None, 0, &store)
            .await
            .unwrap();
        assert!(
            validate_commit_preview(&root, &clean, &store)
                .await
                .unwrap_err()
                .contains("REVIEW_NO_CHANGES")
        );
        std::fs::write(path.join("a"), "changed").unwrap();
        let preview = git
            .compare(&root, CompareMode::Uncommitted, None, 0, &store)
            .await
            .unwrap();
        validate_commit_preview(&root, &preview, &store)
            .await
            .unwrap();
        std::fs::write(path.join("a"), "changed elsewhere").unwrap();
        assert!(
            validate_commit_preview(&root, &preview, &store)
                .await
                .unwrap_err()
                .contains("REVIEW_STALE_COMMIT")
        );
        std::fs::write(path.join("a"), "changed").unwrap();
        require_git_success(
            run_commit_command(
                &path,
                &["commit", "--allow-empty", "-m", "external empty commit"],
            )
            .await
            .unwrap(),
        )
        .unwrap();
        assert!(
            validate_commit_preview(&root, &preview, &store)
                .await
                .unwrap_err()
                .contains("REVIEW_STALE_COMMIT")
        );
        let preview = git
            .compare(&root, CompareMode::Uncommitted, None, 0, &store)
            .await
            .unwrap();
        require_git_success(run_commit_command(&path, &["add", "a"]).await.unwrap()).unwrap();
        assert!(
            validate_commit_preview(&root, &preview, &store)
                .await
                .unwrap_err()
                .contains("REVIEW_STALE_COMMIT")
        );
        let staged = git
            .compare(&root, CompareMode::Staged, None, 0, &store)
            .await
            .unwrap();
        validate_commit_preview(&root, &staged, &store)
            .await
            .unwrap();
        commit_changes(&path, "external commit", false)
            .await
            .unwrap();
        assert!(
            validate_commit_preview(&root, &staged, &store)
                .await
                .unwrap_err()
                .contains("REVIEW_STALE_COMMIT")
        );
        let count = run_commit_command(&path, &["rev-list", "--count", "HEAD"])
            .await
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&count.stdout).trim(), "3");
    }

    #[tokio::test]
    async fn asks_before_staging_and_preserves_partial_staging() {
        let repo = tempfile::tempdir().unwrap();
        let path = repo.path();
        for args in [
            vec!["init"],
            vec!["config", "user.email", "test@example.com"],
            vec!["config", "user.name", "Test"],
            vec!["config", "commit.gpgsign", "false"],
        ] {
            require_git_success(run_commit_command(path, &args).await.unwrap()).unwrap();
        }
        std::fs::write(path.join("a"), "initial").unwrap();
        assert!(!commit_changes(path, "first", false).await.unwrap());
        assert!(
            run_commit_command(path, &["diff", "--cached", "--quiet"])
                .await
                .unwrap()
                .status
                .success()
        );
        assert!(commit_changes(path, "first", true).await.unwrap());
        std::fs::write(path.join("a"), "staged").unwrap();
        require_git_success(run_commit_command(path, &["add", "a"]).await.unwrap()).unwrap();
        std::fs::write(path.join("a"), "unstaged").unwrap();
        std::fs::write(path.join("b"), "untracked").unwrap();
        assert!(commit_changes(path, "second", true).await.unwrap());
        let committed = run_commit_command(path, &["show", "HEAD:a"]).await.unwrap();
        assert_eq!(committed.stdout, b"staged");
        let status = run_commit_command(path, &["status", "--porcelain"])
            .await
            .unwrap();
        let status = String::from_utf8(status.stdout).unwrap();
        assert!(status.contains(" M a"));
        assert!(status.contains("?? b"));
    }
}

#[cfg(test)]
mod svg_preview_tests {
    use super::*;
    #[test]
    fn svg_is_an_image_resource_without_treating_xml_as_an_image() {
        let svg = b"<?xml version=\"1.0\"?><svg xmlns=\"http://www.w3.org/2000/svg\"><path d=\"M0 0\"/></svg>";
        let url = image_data_url(svg, "assets/ICON.SVG").unwrap();
        assert!(url.starts_with("data:image/svg+xml;base64,"));
        assert!(image_data_url(svg, "example.xml").is_none());
        assert!(
            image_data_url(b"\x89PNG\r\n\x1a\n", "image.png")
                .unwrap()
                .starts_with("data:image/png;base64,")
        );
    }
}

#[cfg(all(test, unix))]
mod timeout_tests {
    use super::*;
    #[tokio::test]
    async fn timeout_after_a_real_effect_is_not_reported_as_a_known_failure() {
        let directory = tempfile::tempdir().unwrap();
        let effect = directory.path().join("effect");
        let child = tokio::process::Command::new("/bin/sh")
            .args(["-c", "printf done > \"$1\"; while :; do :; done", "fixture"])
            .arg(&effect)
            .kill_on_drop(true)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while !effect.exists() {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        let error = collect_git_output(child, std::time::Duration::from_millis(20))
            .await
            .unwrap_err();
        assert!(error.starts_with(UNKNOWN_GIT_EFFECT));
        assert_eq!(std::fs::read_to_string(effect).unwrap(), "done");
    }
}
