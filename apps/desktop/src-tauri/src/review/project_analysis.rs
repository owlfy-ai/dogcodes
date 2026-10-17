//! Pass comparison metadata to the Agent; Diff blobs belong to preview/undo only.
use super::ReviewSnapshot;
use super::service::ReviewContext;
use agent_core::turn_review::ReviewRequest;
use serde_json::{Value, json};

pub(crate) async fn project_review_packet(
    state: &ReviewContext<'_>,
    request: &ReviewRequest,
) -> Result<Value, String> {
    let _gate = state.review.gate.lock().await;
    let snapshot = {
        let cache = state
            .review
            .snapshots
            .lock()
            .map_err(|_| "比较缓存不可用")?;
        request
            .source_snapshot
            .as_ref()
            .and_then(|id| cache.get(id))
            .filter(|snapshot| snapshot.task_id == request.task_id.as_str())
            .cloned()
            .ok_or("比较结果已过期或不属于当前任务，请刷新")?
    };
    assemble(&snapshot)
}

fn assemble(snapshot: &ReviewSnapshot) -> Result<Value, String> {
    if snapshot.roots.iter().any(|root| root.error.is_some()) {
        return Err("部分目录读取失败，请刷新比较范围后再审查".into());
    }
    if snapshot.roots.iter().all(|root| root.files.is_empty()) {
        return Err("当前比较范围没有代码改动".into());
    }
    let roots: Vec<_> = snapshot.roots.iter().map(|root| {
        let files: Vec<_> = root.files.iter().map(|file| json!({"path":file.path,"previousPath":file.previous_path,"kind":file.kind})).collect();
        json!({"rootId":root.root.id,"path":root.root.path,"git":root.root.git,"scopes":root.root.scopes,"mode":root.mode,"base":root.base,"target":root.target,"files":files,"excludedRoots":root.excluded_roots})
    }).collect();
    let scopes: Vec<_> = snapshot
        .roots
        .iter()
        .map(|root| {
            let mode = match root.mode {
                super::CompareMode::Uncommitted => "全部未提交",
                super::CompareMode::Unstaged => "未暂存",
                super::CompareMode::Staged => "已暂存",
                super::CompareMode::Committed => "历史提交",
                super::CompareMode::Branch => "分支比较",
                super::CompareMode::Session => "目录已记录修改（当前文件）",
            };
            format!("{} · {}", root.root.path, mode)
        })
        .collect();
    Ok(
        json!({"taskId":snapshot.task_id,"snapshotId":snapshot.id,"scope":scopes.join("；"),"roots":roots}),
    )
}

#[cfg(test)]
mod tests {
    use super::super::{CompareMode, RootChanges};
    use super::*;
    use agent_workspace::review::{
        ChangeKind, FileChange, FileKind, FileVersion, ReviewStore, git::ReviewRoot,
    };
    use std::collections::BTreeMap;

    fn snapshot(store: &ReviewStore) -> ReviewSnapshot {
        let content = "frozen content\n";
        ReviewSnapshot {
            id: "snapshot".into(),
            task_id: "task".into(),
            roots: vec![RootChanges {
                root: ReviewRoot {
                    id: "root".into(),
                    path: "/not/a/live/path".into(),
                    git: true,
                    scopes: vec![],
                },
                mode: CompareMode::Staged,
                base: "base-hash".into(),
                target: "index-hash".into(),
                files: vec![FileChange {
                    path: "new.rs".into(),
                    previous_path: None,
                    kind: ChangeKind::Added,
                    before: None,
                    after: Some(FileVersion {
                        hash: store.put_blob(content.as_bytes()).unwrap(),
                        bytes: content.len() as u64,
                        kind: FileKind::File,
                        executable: false,
                        binary: false,
                    }),
                }],
                untracked: vec![],
                excluded_roots: vec![],
                error: None,
                statistics: BTreeMap::new(),
                text_plans: BTreeMap::new(),
                git_comparison: None,
            }],
        }
    }
    #[test]
    fn project_packet_preserves_scope_without_reading_blobs() {
        let dir = tempfile::tempdir().unwrap();
        let store = ReviewStore::open(dir.path().join("store")).unwrap();
        let mut snapshot = snapshot(&store);
        let mut second = snapshot.roots[0].clone();
        second.root.id = "second".into();
        second.root.path = "/other".into();
        snapshot.roots.push(second);
        let packet = assemble(&snapshot).unwrap();
        assert!(packet["roots"][0]["files"][0].get("after").is_none());
        assert!(!packet.to_string().contains("frozen content"));
        assert_eq!(packet["roots"][1]["rootId"], "second");
        assert_eq!(packet["roots"][0]["mode"], "staged");
        assert_eq!(packet["roots"][0]["base"], "base-hash");
        snapshot.roots[0].files[0].after.as_mut().unwrap().hash = "a".repeat(64);
        assert!(assemble(&snapshot).is_ok());
    }
    #[test]
    fn project_packet_rejects_partial_empty_but_allows_large_and_binary_files() {
        let dir = tempfile::tempdir().unwrap();
        let store = ReviewStore::open(dir.path().join("store")).unwrap();
        let original = snapshot(&store);
        let mut partial = original.clone();
        partial.roots[0].error = Some("read failed".into());
        assert!(assemble(&partial).is_err());
        let mut empty = original.clone();
        empty.roots[0].files.clear();
        assert!(assemble(&empty).is_err());
        let mut oversized = original.clone();
        oversized.roots[0].files[0].after.as_mut().unwrap().bytes = 768_001;
        assert!(assemble(&oversized).is_ok());
        let mut binary = original;
        binary.roots[0].files[0].after.as_mut().unwrap().binary = true;
        assert!(assemble(&binary).is_ok());
    }
}
