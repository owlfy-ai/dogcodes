use super::*;

#[tokio::test]
async fn batch_discard_restores_selected_files_and_rename_without_changing_index() {
    let repo = repository();
    let data = tempfile::tempdir().unwrap();
    let store = ReviewStore::open(data.path().into()).unwrap();
    let service = GitReview::default();
    for path in ["modified", "old", "deleted", "unselected"] {
        fs::write(repo.path().join(path), format!("base for {path}\n")).unwrap();
    }
    git(repo.path(), &["add", "."]);
    git(repo.path(), &["commit", "-m", "base"]);
    fs::write(repo.path().join("modified"), "staged\n").unwrap();
    git(repo.path(), &["add", "modified"]);
    fs::write(repo.path().join("modified"), "working\n").unwrap();
    fs::rename(repo.path().join("old"), repo.path().join("renamed")).unwrap();
    fs::remove_file(repo.path().join("deleted")).unwrap();
    fs::write(repo.path().join("added"), "new\n").unwrap();
    fs::write(repo.path().join("unselected"), "keep\n").unwrap();
    let root = service
        .discover(&[repo.path().to_str().unwrap().into()])
        .await
        .unwrap()
        .remove(0);
    let preview = service
        .compare(&root, CompareMode::Unstaged, None, 0, &store)
        .await
        .unwrap();
    let mut selected: Vec<_> = preview
        .changes
        .iter()
        .filter(|file| file.path != "unselected")
        .cloned()
        .collect();
    for change in &mut selected {
        change.before = service
            .working_replacement(&preview, change, &store)
            .await
            .unwrap();
    }
    let index = fs::read(repo.path().join(".git/index")).unwrap();
    let outcome = store
        .discard_changes("task", repo.path(), &selected)
        .unwrap();
    assert!(outcome.error.is_none());
    assert_eq!(outcome.completed.len(), 4);
    assert_eq!(
        fs::read_to_string(repo.path().join("modified")).unwrap(),
        "staged\n"
    );
    assert_eq!(
        fs::read_to_string(repo.path().join("old")).unwrap(),
        "base for old\n"
    );
    assert_eq!(
        fs::read_to_string(repo.path().join("deleted")).unwrap(),
        "base for deleted\n"
    );
    assert_eq!(
        fs::read_to_string(repo.path().join("unselected")).unwrap(),
        "keep\n"
    );
    assert!(!repo.path().join("added").exists());
    assert!(!repo.path().join("renamed").exists());
    assert_eq!(fs::read(repo.path().join(".git/index")).unwrap(), index);
    let records = store.recovery_entries("task").unwrap();
    assert_eq!(records.len(), 3);
    assert!(records.iter().all(|entry| entry.applied));
    assert_eq!(
        store
            .read_blob(
                &records
                    .iter()
                    .find(|entry| entry.record.path == "modified")
                    .unwrap()
                    .record
                    .original
                    .as_ref()
                    .unwrap()
                    .hash
            )
            .unwrap(),
        b"working\n"
    );
}

#[tokio::test]
async fn batch_discard_stale_or_invalid_member_does_not_change_any_file() {
    let repo = repository();
    let data = tempfile::tempdir().unwrap();
    let store = ReviewStore::open(data.path().into()).unwrap();
    let service = GitReview::default();
    for path in ["a", "b"] {
        fs::write(repo.path().join(path), format!("base {path}")).unwrap();
    }
    git(repo.path(), &["add", "."]);
    git(repo.path(), &["commit", "-m", "base"]);
    for path in ["a", "b"] {
        fs::write(repo.path().join(path), format!("working {path}")).unwrap();
    }
    let root = service
        .discover(&[repo.path().to_str().unwrap().into()])
        .await
        .unwrap()
        .remove(0);
    let preview = service
        .compare(&root, CompareMode::Uncommitted, None, 0, &store)
        .await
        .unwrap();
    fs::write(repo.path().join("b"), "newer edit").unwrap();
    assert!(
        store
            .discard_changes("task", repo.path(), &preview.changes)
            .is_err()
    );
    assert_eq!(
        fs::read_to_string(repo.path().join("a")).unwrap(),
        "working a"
    );
    assert_eq!(
        fs::read_to_string(repo.path().join("b")).unwrap(),
        "newer edit"
    );
    fs::write(repo.path().join("b"), "working b").unwrap();
    let mut broken = preview.changes.clone();
    broken
        .iter_mut()
        .find(|file| file.path == "b")
        .unwrap()
        .before
        .as_mut()
        .unwrap()
        .hash = "missing".into();
    assert!(store.discard_changes("task", repo.path(), &broken).is_err());
    assert_eq!(
        fs::read_to_string(repo.path().join("a")).unwrap(),
        "working a"
    );
    assert_eq!(
        fs::read_to_string(repo.path().join("b")).unwrap(),
        "working b"
    );
    assert!(store.discard_changes("task", repo.path(), &[]).is_err());
    assert!(
        store
            .discard_changes(
                "task",
                repo.path(),
                &[preview.changes[0].clone(), preview.changes[0].clone()]
            )
            .is_err()
    );
}
