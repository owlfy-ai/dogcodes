use super::*;

#[tokio::test]
async fn file_recovery_lock_validates_scope_and_releases_only_its_lock() {
    let directory = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let service = GitReview::default();
    let store = ReviewStore::open(data.path().into()).unwrap();
    service
        .text(directory.path(), &["init", "-q"])
        .await
        .unwrap();
    let record = store
        .prepare_recovery(
            "task",
            directory.path(),
            "missing",
            None,
            Some(crate::review::FileVersion {
                hash: store.put_blob(b"restored").unwrap(),
                bytes: 8,
                kind: FileKind::File,
                executable: false,
                binary: false,
            }),
        )
        .unwrap();
    let request = |task| FileRecoveryLockRequest {
        task,
        record: &record.id,
        grouped: false,
    };
    assert!(
        service
            .lock_file_recovery(directory.path(), request("wrong"), &store)
            .await
            .is_err()
    );
    assert!(
        service
            .lock_file_recovery(data.path(), request("task"), &store)
            .await
            .is_err()
    );
    let guard = service
        .lock_file_recovery(directory.path(), request("task"), &store)
        .await
        .unwrap();
    let git = directory.path().join(".git");
    assert!(git.join("index.lock").is_file());
    drop(guard);
    assert!(!git.join("index.lock").exists());
}

#[tokio::test]
async fn first_stage_prepares_recovery_without_overwriting_existing_lock() {
    let directory = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let service = GitReview::default();
    let store = ReviewStore::open(data.path().into()).unwrap();
    service
        .text(directory.path(), &["init", "-q"])
        .await
        .unwrap();
    fs::write(directory.path().join("sample"), b"reviewed bytes").unwrap();
    let roots = service
        .discover(&[directory.path().to_string_lossy().into_owned()])
        .await
        .unwrap();
    let comparison = service
        .compare(&roots[0], CompareMode::Unstaged, None, 0, &store)
        .await
        .unwrap();
    let index = service.git_path(directory.path(), "index").await.unwrap();
    let lock = index.with_file_name("index.lock");
    fs::write(&lock, b"another Git operation").unwrap();
    assert!(
        service
            .change_index("task", &comparison, "sample", IndexAction::Stage, &store)
            .await
            .is_err()
    );
    assert_eq!(fs::read(&lock).unwrap(), b"another Git operation");
    assert!(!index.exists());
    assert_eq!(
        fs::read(directory.path().join("sample")).unwrap(),
        b"reviewed bytes"
    );
    let records = store.index_recovery_entries("task").unwrap();
    assert_eq!(
        records.len(),
        1,
        "fixed intent exists before Git lock acquisition"
    );
}

#[test]
fn index_reads_reject_non_regular_and_oversized_files() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("index");
    assert_eq!(read_index(&path).unwrap(), None);
    assert!(read_index(directory.path()).is_err());
    fs::write(&path, b"valid bytes").unwrap();
    assert_eq!(read_index(&path).unwrap().unwrap(), b"valid bytes");
    fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(32 * 1024 * 1024 + 1)
        .unwrap();
    assert!(read_index(&path).is_err());
}

#[cfg(unix)]
#[test]
fn index_read_detects_path_replacement_without_reading_foreign_target() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("index");
    fs::write(&path, b"original").unwrap();
    let before = fs::symlink_metadata(&path).unwrap();
    let file = fs::File::open(&path).unwrap();
    fs::rename(&path, directory.path().join("old-index")).unwrap();
    let foreign = directory.path().join("foreign");
    fs::write(&foreign, b"foreign bytes").unwrap();
    std::os::unix::fs::symlink(&foreign, &path).unwrap();
    assert!(read_index_handle(&path, file, &before).is_err());
    assert!(read_index(&path).is_err());
    assert_eq!(fs::read(&foreign).unwrap(), b"foreign bytes");
}

#[test]
fn index_read_detects_growth_since_path_inspection() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("index");
    fs::write(&path, b"before").unwrap();
    let before = fs::symlink_metadata(&path).unwrap();
    let file = fs::File::open(&path).unwrap();
    fs::write(&path, b"larger after").unwrap();
    assert!(read_index_handle(&path, file, &before).is_err());
}

#[tokio::test]
async fn independent_index_resume_requires_confirmation_and_preserves_new_work() {
    use crate::review::recovery::index::IndexRecoveryContent;
    let root = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let store = ReviewStore::open(data.path().into()).unwrap();
    let service = GitReview::default();
    service.text(root.path(), &["init", "-q"]).await.unwrap();
    fs::write(root.path().join("file"), b"staged version").unwrap();
    service
        .text(root.path(), &["add", "--", "file"])
        .await
        .unwrap();
    let index = service.git_path(root.path(), "index").await.unwrap();
    let candidate = fs::read(&index).unwrap();
    fs::remove_file(&index).unwrap();
    let id = store
        .prepare_index_recovery(
            "task",
            root.path(),
            vec!["file".into()],
            true,
            IndexRecoveryContent {
                original: None,
                candidate: &candidate,
                repository: service.repository_version(root.path()).await.unwrap(),
            },
        )
        .unwrap();
    assert!(
        service
            .resume_index_recovery("task", &id, &store, false)
            .await
            .is_err()
    );
    assert!(!index.exists());
    let head = service.git_path(root.path(), "HEAD").await.unwrap();
    let old_head = fs::read(&head).unwrap();
    fs::write(&head, b"ref: refs/heads/changed\n").unwrap();
    assert!(
        service
            .resume_index_recovery("task", &id, &store, true)
            .await
            .is_err()
    );
    assert!(!index.exists());
    fs::write(&head, old_head).unwrap();
    fs::write(&index, b"foreign").unwrap();
    assert!(
        service
            .resume_index_recovery("task", &id, &store, true)
            .await
            .is_err()
    );
    assert_eq!(fs::read(&index).unwrap(), b"foreign");
    fs::remove_file(&index).unwrap();
    fs::write(root.path().join("file"), b"later working edit").unwrap();
    service
        .resume_index_recovery("task", &id, &store, true)
        .await
        .unwrap();
    assert_eq!(fs::read(&index).unwrap(), candidate);
    assert_eq!(
        fs::read(root.path().join("file")).unwrap(),
        b"later working edit"
    );
    let marker = data
        .path()
        .join("index-recovery")
        .join(format!("{id}.applied"));
    fs::remove_file(marker).unwrap(); // Publication survived but completion did not.
    service
        .resume_index_recovery("task", &id, &store, true)
        .await
        .unwrap();
    assert_eq!(fs::read(&index).unwrap(), candidate);
    fs::write(&index, b"newer completed operation").unwrap();
    service
        .resume_index_recovery("task", &id, &store, true)
        .await
        .unwrap();
    assert_eq!(fs::read(&index).unwrap(), b"newer completed operation");
    assert!(!index.with_file_name("index.lock").exists());
}

#[tokio::test]
async fn independent_index_observation_reads_repository_without_publishing() {
    use crate::review::recovery::index::IndexRecoveryState;
    let root = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let store = ReviewStore::open(data.path().into()).unwrap();
    let service = GitReview::default();
    service.text(root.path(), &["init", "-q"]).await.unwrap();
    let index = service.git_path(root.path(), "index").await.unwrap();
    let id = store
        .prepare_index_recovery(
            "task",
            root.path(),
            vec!["file".into()],
            true,
            crate::review::recovery::index::IndexRecoveryContent {
                original: None,
                candidate: b"candidate",
                repository: service.repository_version(root.path()).await.unwrap(),
            },
        )
        .unwrap();
    assert_eq!(
        service
            .inspect_index_recovery("task", &id, &store)
            .await
            .unwrap(),
        IndexRecoveryState::Prepared
    );
    assert!(!index.exists());
    fs::write(&index, b"candidate").unwrap();
    assert_eq!(
        service
            .inspect_index_recovery("task", &id, &store)
            .await
            .unwrap(),
        IndexRecoveryState::Published
    );
    fs::write(&index, b"foreign").unwrap();
    assert_eq!(
        service
            .inspect_index_recovery("task", &id, &store)
            .await
            .unwrap(),
        IndexRecoveryState::Diverged
    );
    assert_eq!(fs::read(&index).unwrap(), b"foreign");
    assert!(!index.with_file_name("index.lock").exists());
}

#[tokio::test]
async fn working_guard_preserves_replaced_parent_index_lock() {
    let root = tempfile::tempdir().unwrap();
    let service = GitReview::default();
    service.text(root.path(), &["init", "-q"]).await.unwrap();
    let index = service.git_path(root.path(), "index").await.unwrap();
    fs::write(&index, b"original index").unwrap();
    let lock_path = index.with_file_name("index.lock");
    let guard = service.lock_working(root.path()).await.unwrap();
    assert!(service.lock_working(root.path()).await.is_err());
    fs::remove_file(&lock_path).unwrap();
    fs::write(&lock_path, b"foreign index lock").unwrap();
    drop(guard);
    assert_eq!(fs::read(&lock_path).unwrap(), b"foreign index lock");
    assert_eq!(fs::read(&index).unwrap(), b"original index");
    fs::remove_file(&lock_path).unwrap();
    drop(service.lock_working(root.path()).await.unwrap());
    assert!(!lock_path.exists());
}

#[cfg(unix)]
#[tokio::test]
async fn working_guard_preserves_foreign_symlink_and_target() {
    let root = tempfile::tempdir().unwrap();
    let service = GitReview::default();
    service.text(root.path(), &["init", "-q"]).await.unwrap();
    let index = service.git_path(root.path(), "index").await.unwrap();
    let guard = service.lock_working(root.path()).await.unwrap();
    let lock_path = index.with_file_name("index.lock");
    let foreign = root.path().join("foreign");
    fs::write(&foreign, b"untouched").unwrap();
    fs::remove_file(&lock_path).unwrap();
    std::os::unix::fs::symlink(&foreign, &lock_path).unwrap();
    drop(guard);
    assert!(fs::symlink_metadata(&lock_path).unwrap().is_symlink());
    assert_eq!(fs::read(&foreign).unwrap(), b"untouched");
    assert!(!index.exists());
}
#[test]
fn head_read_is_bounded_and_preserves_exact_bytes() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("HEAD");
    assert!(super::read_head(&path).is_err());
    for bytes in [b"ref: refs/heads/main\r\n".to_vec(), vec![b'a'; 4096]] {
        std::fs::write(&path, &bytes).unwrap();
        assert_eq!(super::read_head(&path).unwrap(), bytes);
    }
    std::fs::write(&path, vec![b'a'; 4097]).unwrap();
    assert!(super::read_head(&path).is_err());
    assert!(super::read_head(directory.path()).is_err());
}

#[cfg(unix)]
#[test]
fn head_read_rejects_symlink_without_modifying_target() {
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("original");
    let path = directory.path().join("HEAD");
    std::fs::write(&target, b"ref: refs/heads/main\n").unwrap();
    std::os::unix::fs::symlink(&target, &path).unwrap();
    assert!(super::read_head(&path).is_err());
    assert_eq!(std::fs::read(&target).unwrap(), b"ref: refs/heads/main\n");
}
