//! Real process death, not a simulated Rust owner drop. Only temporary fixture data is used.
use agent_workspace::review::ReviewStore;
use sha2::Digest;
use std::{
    fs,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

struct TestProcess(Child);
impl Drop for TestProcess {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
#[ignore = "fixture subprocess, invoked by killed_preview_owner_is_reclaimable"]
fn preview_owner_worker() {
    let directory = std::path::PathBuf::from(
        std::env::var_os("DOGCODES_REVIEW_CRASH_FIXTURE").expect("fixture directory"),
    );
    let store = ReviewStore::open(directory.clone()).unwrap();
    let owner = store.create_snapshot_owner("crash-fixture-owner").unwrap();
    let transient = store.put_blob(b"preview-only").unwrap();
    let durable = store.put_blob(b"recovery-backup").unwrap();
    store
        .persist_owned_snapshot_references(
            &owner,
            "submodule-backup/task/token",
            [transient, durable.clone()],
        )
        .unwrap();
    store
        .persist_snapshot_references("submodule-recovery/record", [durable])
        .unwrap();
    drop(store);
    fs::write(directory.join("ready.tmp"), b"ready").unwrap();
    fs::rename(directory.join("ready.tmp"), directory.join("ready")).unwrap();
    // Keep the owner lock alive until the parent forcefully terminates this exact child.
    loop {
        thread::park();
        std::hint::black_box(&owner);
    }
}

#[test]
fn killed_preview_owner_is_reclaimable() {
    let directory = tempfile::tempdir().unwrap();
    let mut child = TestProcess(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "preview_owner_worker",
                "--nocapture",
            ])
            .env("DOGCODES_REVIEW_CRASH_FIXTURE", directory.path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while !directory.path().join("ready").exists() {
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "fixture exited before readiness"
        );
        assert!(Instant::now() < deadline, "fixture readiness timed out");
        thread::sleep(Duration::from_millis(10));
    }
    let maintenance = ReviewStore::try_open_maintenance(directory.path().into()).unwrap();
    assert_eq!(maintenance.release_abandoned_snapshots().unwrap(), 0);
    assert_eq!(maintenance.storage_usage([]).unwrap().protected_bytes, 27);
    child.0.kill().unwrap();
    assert!(!child.0.wait().unwrap().success());
    assert_eq!(maintenance.release_abandoned_snapshots().unwrap(), 1);
    assert_eq!(maintenance.release_abandoned_snapshots().unwrap(), 0);
    assert_eq!(maintenance.storage_usage([]).unwrap().protected_bytes, 15);
    drop(maintenance);
    let reopened = ReviewStore::try_open_maintenance(directory.path().into()).unwrap();
    assert_eq!(reopened.storage_usage([]).unwrap().protected_bytes, 15);
    assert_eq!(reopened.release_abandoned_snapshots().unwrap(), 0);
    for bytes in [b"preview-only".as_slice(), b"recovery-backup".as_slice()] {
        let hash = hex::encode(sha2::Sha256::digest(bytes));
        assert_eq!(reopened.read_blob(&hash).unwrap(), bytes);
    }
}
