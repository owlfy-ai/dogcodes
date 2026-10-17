use super::*;
static PARTIAL_WORKERS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

pub(super) fn checkpoint(store: &ReviewStore, phase: &str) {
    let Some(directory) = std::env::var_os("DOGCODES_SCAN_CRASH_FIXTURE") else {
        return;
    };
    let directory = PathBuf::from(directory);
    if store.directory != directory.join("store")
        || std::env::var("DOGCODES_SCAN_CRASH_PHASE").as_deref() != Ok(phase)
    {
        return;
    }
    // The partial fixture has 64 objects / four workers. Wait until each
    // finished its first write so the checkpoint never races temporary files.
    if phase != "partial" || PARTIAL_WORKERS.fetch_add(1, Ordering::SeqCst) == 3 {
        fs::write(directory.join("ready"), phase).unwrap();
    }
    loop {
        std::thread::park();
    }
}

#[test]
#[ignore = "isolated worker for the scan batch crash tests"]
fn scan_crash_worker() {
    let Some(directory) = std::env::var_os("DOGCODES_SCAN_CRASH_FIXTURE") else {
        return;
    };
    let directory = PathBuf::from(directory);
    let store = ReviewStore::open(directory.join("store")).unwrap();
    store
        .ensure_baseline("task", &directory.join("root"), SnapshotBudget::default())
        .unwrap();
    panic!("worker did not reach requested checkpoint");
}

#[test]
fn killed_scan_after_reservation_recounts_without_publishing_baseline() {
    killed_scan("reserved", 0);
}

#[test]
fn killed_scan_after_blob_publication_recounts_without_publishing_baseline() {
    killed_scan("published", 64);
}

#[test]
fn killed_scan_during_parallel_publication_recounts_without_publishing_baseline() {
    killed_scan("partial", 0);
}

fn killed_scan(phase: &str, published: usize) {
    use std::process::{Child, Command, Stdio};
    struct Worker(Child);
    impl Drop for Worker {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let fixture = tempfile::tempdir().unwrap();
    let directory = fixture.path();
    let root = directory.join("root");
    fs::create_dir(&root).unwrap();
    for file in 0_u8..64 {
        fs::write(root.join(format!("file-{file}")), [file]).unwrap();
    }
    let store = ReviewStore::open(directory.join("store")).unwrap();
    store.put_blob(b"seed").unwrap(); // Pre-populate a cache that the child invalidates.
    let mut worker = Worker(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "review::scan_crash_tests::scan_crash_worker",
                "--ignored",
            ])
            .env("DOGCODES_SCAN_CRASH_FIXTURE", directory)
            .env("DOGCODES_SCAN_CRASH_PHASE", phase)
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while !directory.join("ready").exists() {
        assert!(
            worker.0.try_wait().unwrap().is_none(),
            "worker exited before checkpoint"
        );
        assert!(Instant::now() < deadline, "worker checkpoint timed out");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        store.put_blob(b"blocked").is_err(),
        "live batch still owns quota lock"
    );
    worker.0.kill().unwrap();
    worker.0.wait().unwrap();
    assert!(store.load_baseline("task", &root).is_err());
    let published = if phase == "partial" {
        let count = fs::read_dir(directory.join("store/blobs")).unwrap().count() - 1;
        assert_eq!(count, 4, "each worker pauses after its first object");
        count
    } else {
        published
    };
    assert_eq!(
        fs::read_dir(directory.join("store/blobs")).unwrap().count(),
        published + 1
    );
    let total = 4 + published as u64;
    assert!(
        store.put_blob_with_capacity(b"new", total).is_err(),
        "must recount child's published bytes"
    );
    store.put_blob_with_capacity(b"new", total + 3).unwrap();
    assert_eq!(store.quota_cache.lock().unwrap().unwrap().1, total + 3);
    assert_eq!(store.quota_cache.lock().unwrap().unwrap().2, published + 2);
    drop(store);
    let reopened = ReviewStore::open(directory.join("store")).unwrap();
    let baseline = reopened
        .ensure_baseline("task", &root, SnapshotBudget::default())
        .unwrap();
    assert!(baseline.complete);
    assert_eq!(baseline.files.len(), 64);
    for file in 0_u8..64 {
        let version = &baseline.files[&format!("file-{file}")];
        assert_eq!(reopened.read_blob(&version.hash).unwrap(), [file]);
        assert_eq!(fs::read(root.join(format!("file-{file}"))).unwrap(), [file]);
    }
    assert_eq!(
        fs::read_dir(directory.join("store/blobs")).unwrap().count(),
        66
    );
    assert!(!root.join(".git").exists());
}
