use super::*;

fn content<'a>(original: Option<&'a [u8]>, candidate: &'a [u8]) -> IndexRecoveryContent<'a> {
    IndexRecoveryContent {
        original,
        candidate,
        repository: RepositoryVersion {
            head: None,
            head_file: String::new(),
            index: original.map(digest),
        },
    }
}

#[test]
fn index_listing_is_task_scoped_and_rejects_corrupt_manifests() {
    let data = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let store = ReviewStore::open(data.path().into()).unwrap();
    assert!(store.index_recovery_entries("task").unwrap().is_empty());
    let first = store
        .prepare_index_recovery(
            "task",
            root.path(),
            vec!["a".into()],
            true,
            content(None, b"one"),
        )
        .unwrap();
    let other = store
        .prepare_index_recovery(
            "other",
            root.path(),
            vec!["b".into()],
            false,
            content(None, b"two"),
        )
        .unwrap();
    store.complete_index_recovery("task", &first).unwrap();
    let entries = store.index_recovery_entries("task").unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].0, first);
    assert_eq!(entries[0].1.paths, ["a"]);
    assert_eq!(store.index_recovery_entries("other").unwrap()[0].0, other);
    fs::write(data.path().join("index-recovery").join(&first), b"{}").unwrap();
    assert!(store.index_recovery_entries("task").is_err());
}

#[cfg(unix)]
#[test]
fn index_listing_rejects_manifest_symlinks() {
    let data = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let store = ReviewStore::open(data.path().into()).unwrap();
    let id = store
        .prepare_index_recovery(
            "task",
            root.path(),
            vec!["a".into()],
            true,
            content(None, b"one"),
        )
        .unwrap();
    let path = data.path().join("index-recovery").join(&id);
    let target = root.path().join("manifest");
    fs::rename(&path, &target).unwrap();
    std::os::unix::fs::symlink(&target, &path).unwrap();
    assert!(store.index_recovery_entries("task").is_err());
    assert!(target.exists());
}

#[test]
fn index_state_distinguishes_publication_from_later_edits_without_writing() {
    let data = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let store = ReviewStore::open(data.path().into()).unwrap();
    for original in [None, Some(b"old".as_slice())] {
        let id = store
            .prepare_index_recovery(
                "task",
                root.path(),
                vec!["file".into()],
                true,
                content(original, b"new"),
            )
            .unwrap();
        assert_eq!(
            store.index_recovery_state("task", &id, original).unwrap(),
            IndexRecoveryState::Prepared
        );
        assert_eq!(
            store
                .index_recovery_state("task", &id, Some(b"new"))
                .unwrap(),
            IndexRecoveryState::Published
        );
        assert_eq!(
            store
                .index_recovery_state("task", &id, Some(b"foreign"))
                .unwrap(),
            IndexRecoveryState::Diverged
        );
        assert!(store.index_recovery_state("other", &id, original).is_err());
        let marker = data
            .path()
            .join("index-recovery")
            .join(format!("{id}.applied"));
        assert!(!marker.exists());
        store.complete_index_recovery("task", &id).unwrap();
        // Later changes (including returning to original) cannot revive history.
        assert_eq!(
            store.index_recovery_state("task", &id, original).unwrap(),
            IndexRecoveryState::Applied
        );
        assert_eq!(
            store
                .index_recovery_state("task", &id, Some(b"foreign"))
                .unwrap(),
            IndexRecoveryState::Applied
        );
        fs::write(&marker, b"invalid").unwrap();
        assert!(store.index_recovery_state("task", &id, original).is_err());
    }
}

#[test]
fn index_intent_survives_reopen_protects_bytes_and_separates_repeated_operations() {
    let data = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let store = ReviewStore::open(data.path().into()).unwrap();
    let first = store
        .prepare_index_recovery(
            "task",
            root.path(),
            vec!["old".into(), "new".into()],
            true,
            content(Some(b"original index"), b"candidate index"),
        )
        .unwrap();
    let second = store
        .prepare_index_recovery(
            "task",
            root.path(),
            vec!["old".into(), "new".into()],
            true,
            content(Some(b"original index"), b"candidate index"),
        )
        .unwrap();
    assert_ne!(first, second);
    assert!(
        !data
            .path()
            .join("index-recovery")
            .join(format!("{first}.applied"))
            .exists()
    );
    drop(store);
    let store = ReviewStore::open(data.path().into()).unwrap();
    assert!(store.index_recovery("other-task", &first).is_err());
    let record = store.index_recovery("task", &first).unwrap();
    assert_eq!(
        store
            .read_blob(record.original.as_deref().unwrap())
            .unwrap(),
        b"original index"
    );
    assert_eq!(
        store.read_blob(&record.candidate).unwrap(),
        b"candidate index"
    );
    assert_eq!(store.storage_usage([]).unwrap().protected_bytes, 29);
    store.complete_index_recovery("task", &first).unwrap();
    assert!(
        data.path()
            .join("index-recovery")
            .join(format!("{first}.applied"))
            .exists()
    );
    assert!(
        !data
            .path()
            .join("index-recovery")
            .join(format!("{second}.applied"))
            .exists()
    );
}

#[test]
fn batch_index_manifest_round_trips_long_path_lists_with_a_bounded_budget() {
    let data = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let store = ReviewStore::open(data.path().into()).unwrap();
    let paths: Vec<_> = (0..2000)
        .map(|index| format!("packages/project/src/components/nested/file-{index}.tsx"))
        .collect();
    let id = store
        .prepare_index_recovery(
            "task",
            root.path(),
            paths.clone(),
            true,
            content(None, b"index"),
        )
        .unwrap();
    assert!(
        fs::metadata(data.path().join("index-recovery").join(&id))
            .unwrap()
            .len()
            > 65536
    );
    assert_eq!(store.index_recovery("task", &id).unwrap().paths, paths);
    store.complete_index_recovery("task", &id).unwrap();
    assert!(
        store
            .prepare_index_recovery(
                "task",
                root.path(),
                vec!["a".into(); MAX_INDEX_RECOVERY_PATHS + 1],
                true,
                content(None, b"index")
            )
            .is_err()
    );
}
