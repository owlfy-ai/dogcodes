use super::*;
use operations::IndexAction;

#[tokio::test]
async fn batch_index_stages_and_unstages_only_selected_files_including_renames() {
    let repo = repository();
    let data = tempfile::tempdir().unwrap();
    let store = ReviewStore::open(data.path().into()).unwrap();
    let service = GitReview::default();
    for path in ["a", "old", "deleted", "untouched"] {
        fs::write(repo.path().join(path), format!("base for {path}\n")).unwrap();
    }
    git(repo.path(), &["add", "."]);
    git(repo.path(), &["commit", "-m", "base"]);
    fs::write(repo.path().join("a"), "changed\n").unwrap();
    fs::rename(repo.path().join("old"), repo.path().join("new")).unwrap();
    fs::remove_file(repo.path().join("deleted")).unwrap();
    fs::write(repo.path().join("added"), "new file\n").unwrap();
    fs::write(repo.path().join("untouched"), "already staged\n").unwrap();
    git(repo.path(), &["add", "untouched"]);
    fs::write(repo.path().join("untouched"), "unstaged after staged\n").unwrap();
    let root = service
        .discover(&[repo.path().to_str().unwrap().into()])
        .await
        .unwrap()
        .remove(0);
    let preview = service
        .compare(&root, CompareMode::Unstaged, None, 0, &store)
        .await
        .unwrap();
    let paths = ["a", "new", "deleted", "added", "a"].map(String::from);
    service
        .change_index_many("task", &preview, &paths, IndexAction::Stage, &store)
        .await
        .unwrap();
    assert_eq!(git(repo.path(), &["show", ":a"]), "changed");
    assert_eq!(git(repo.path(), &["ls-files"]), "a\nadded\nnew\nuntouched");
    assert_eq!(git(repo.path(), &["show", ":untouched"]), "already staged");
    let staged = service
        .compare(&root, CompareMode::Staged, None, 0, &store)
        .await
        .unwrap();
    service
        .change_index_many("task", &staged, &paths, IndexAction::Unstage, &store)
        .await
        .unwrap();
    assert_eq!(
        git(repo.path(), &["diff", "--cached", "--name-only"]),
        "untouched"
    );
    assert_eq!(
        git(repo.path(), &["ls-files"]),
        "a\ndeleted\nold\nuntouched"
    );
    assert_eq!(
        fs::read_to_string(repo.path().join("a")).unwrap(),
        "changed\n"
    );
    assert!(repo.path().join("new").exists());
    assert!(!repo.path().join("old").exists());
    assert!(!repo.path().join("deleted").exists());
    let records = store.index_recovery_entries("task").unwrap();
    assert_eq!(records.len(), 2);
    assert!(records.iter().all(|(_, record)| record.paths.len() == 5));
}

#[tokio::test]
async fn batch_index_invalid_selection_and_stale_preview_leave_index_unchanged() {
    let repo = repository();
    let data = tempfile::tempdir().unwrap();
    let store = ReviewStore::open(data.path().into()).unwrap();
    let service = GitReview::default();
    fs::write(repo.path().join("a"), "base").unwrap();
    git(repo.path(), &["add", "."]);
    git(repo.path(), &["commit", "-m", "base"]);
    fs::write(repo.path().join("a"), "changed").unwrap();
    fs::write(repo.path().join("b"), "new").unwrap();
    let root = service
        .discover(&[repo.path().to_str().unwrap().into()])
        .await
        .unwrap()
        .remove(0);
    let preview = service
        .compare(&root, CompareMode::Unstaged, None, 0, &store)
        .await
        .unwrap();
    let index = fs::read(repo.path().join(".git/index")).unwrap();
    for paths in [
        vec![],
        vec!["a".into(), "missing".into()],
        vec!["a".into(), "../outside".into()],
    ] {
        assert!(
            service
                .change_index_many("task", &preview, &paths, IndexAction::Stage, &store)
                .await
                .is_err()
        );
        assert_eq!(fs::read(repo.path().join(".git/index")).unwrap(), index);
    }
    fs::write(repo.path().join("b"), "concurrent edit").unwrap();
    assert!(
        service
            .change_index_many(
                "task",
                &preview,
                &["a".into(), "b".into()],
                IndexAction::Stage,
                &store
            )
            .await
            .is_err()
    );
    assert_eq!(fs::read(repo.path().join(".git/index")).unwrap(), index);
}

#[tokio::test]
async fn batch_index_preparation_failure_does_not_publish_a_partial_candidate() {
    let repo = repository();
    let data = tempfile::tempdir().unwrap();
    let store = ReviewStore::open(data.path().into()).unwrap();
    let service = GitReview::default();
    fs::write(repo.path().join("a"), "base").unwrap();
    git(repo.path(), &["add", "."]);
    git(repo.path(), &["commit", "-m", "base"]);
    fs::write(repo.path().join("a"), "changed").unwrap();
    fs::write(repo.path().join("b"), "new").unwrap();
    let root = service
        .discover(&[repo.path().to_str().unwrap().into()])
        .await
        .unwrap()
        .remove(0);
    let mut preview = service
        .compare(&root, CompareMode::Unstaged, None, 0, &store)
        .await
        .unwrap();
    preview
        .changes
        .iter_mut()
        .find(|file| file.path == "b")
        .unwrap()
        .after
        .as_mut()
        .unwrap()
        .hash = "missing-blob".into();
    let index = fs::read(repo.path().join(".git/index")).unwrap();
    assert!(
        service
            .change_index_many(
                "task",
                &preview,
                &["a".into(), "b".into()],
                IndexAction::Stage,
                &store
            )
            .await
            .is_err()
    );
    assert_eq!(fs::read(repo.path().join(".git/index")).unwrap(), index);
    assert!(!repo.path().join(".git/index.lock").exists());
}
