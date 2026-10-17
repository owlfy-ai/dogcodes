//! Opt-in quantitative fixture, not a timing-sensitive CI assertion.
use super::*;
use std::{
    fmt::Write as _,
    io::Write as _,
    process::{Command, Stdio},
};

#[test]
#[ignore = "quantitative non-Git baseline and multi-edit scan; run explicitly"]
fn review_non_git_session_performance() {
    use crate::review::SnapshotBudget;
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().join("notes");
    fs::create_dir(&root).unwrap();
    for file in 0..2000 {
        fs::write(
            root.join(format!("file-{file}.txt")),
            format!("original {file}\n"),
        )
        .unwrap();
    }
    let store_path = fixture.path().join("store");
    let store = ReviewStore::open(store_path.clone()).unwrap();
    let started = Instant::now();
    let baseline = store
        .ensure_baseline("session", &root, SnapshotBudget::default())
        .unwrap();
    let baseline_ms = started.elapsed().as_secs_f64() * 1000.0;
    assert!(
        baseline.complete,
        "baseline incomplete after {baseline_ms:.2} ms: {} of 2000 files; diagnostics: {:?}",
        baseline.files.len(),
        baseline.diagnostics
    );
    assert_eq!(baseline.files.len(), 2000);
    let baseline_bytes = serde_json::to_vec(&baseline).unwrap();
    let mut rounds = Vec::new();
    for round in 0..5 {
        for file in 0..100 {
            fs::write(
                root.join(format!("file-{file}.txt")),
                format!("round {round}, file {file}\n"),
            )
            .unwrap();
        }
        let started = Instant::now();
        let current = store.scan(&root, SnapshotBudget::default()).unwrap();
        let scan_ms = started.elapsed().as_secs_f64() * 1000.0;
        assert!(current.complete);
        let started = Instant::now();
        let changes = compare_snapshots(&baseline, &current).unwrap();
        let compare_ms = started.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(changes.len(), 100);
        for change in &changes {
            assert_eq!(change.before.as_ref(), baseline.files.get(&change.path));
        }
        assert_eq!(
            serde_json::to_vec(
                &store
                    .ensure_baseline("session", &root, SnapshotBudget::default())
                    .unwrap()
            )
            .unwrap(),
            baseline_bytes
        );
        // Repeated, unchanged scans must not add CAS versions.
        let count = fs::read_dir(store_path.join("blobs")).unwrap().count();
        let repeated = store.scan(&root, SnapshotBudget::default()).unwrap();
        assert!(compare_snapshots(&current, &repeated).unwrap().is_empty());
        assert_eq!(
            fs::read_dir(store_path.join("blobs")).unwrap().count(),
            count
        );
        rounds.push(serde_json::json!({"round": round, "scanMs": scan_ms, "compareMs": compare_ms, "blobs": count}));
    }
    let started = Instant::now();
    let usage = store.storage_usage([]).unwrap();
    let inventory_ms = started.elapsed().as_secs_f64() * 1000.0;
    assert_eq!(usage.blob_count, 2500);
    assert_eq!(usage.missing_references, 0);
    assert!(usage.protected_bytes > 0 && usage.unreferenced_bytes > 0);
    drop(store);
    let reopened = ReviewStore::open(store_path).unwrap();
    assert_eq!(
        serde_json::to_vec(&reopened.load_baseline("session", &root).unwrap()).unwrap(),
        baseline_bytes
    );
    assert!(!root.join(".git").exists());
    println!(
        "REVIEW_NON_GIT_PERFORMANCE {}",
        serde_json::json!({"os": std::env::consts::OS, "arch": std::env::consts::ARCH, "files": 2000, "changedPerRound": 100, "baselineMs": baseline_ms, "inventoryMs": inventory_ms, "totalBytes": usage.total_bytes, "protectedBytes": usage.protected_bytes, "unreferencedBytes": usage.unreferenced_bytes, "rounds": rounds})
    );
}

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
#[ignore = "quantitative 1000 unique changed files; run explicitly with --nocapture"]
async fn review_dense_changes_performance() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().join("repo");
    fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q"]);
    for file in 0..1000 {
        fs::write(
            root.join(format!("file-{file:04}.txt")),
            format!("base {file}\n"),
        )
        .unwrap();
    }
    git(&root, &["add", "."]);
    git(
        &root,
        &[
            "-c",
            "user.name=Review Test",
            "-c",
            "user.email=review@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "base",
        ],
    );
    for file in 0..1000 {
        fs::write(
            root.join(format!("file-{file:04}.txt")),
            format!("staged {file}\n"),
        )
        .unwrap();
    }
    git(&root, &["add", "."]);
    for file in 0..1000 {
        fs::write(
            root.join(format!("file-{file:04}.txt")),
            format!("working {file}\n"),
        )
        .unwrap();
    }
    let index = fs::read(root.join(".git/index")).unwrap();
    let service = GitReview::default();
    let scope = service
        .discover(&[root.to_string_lossy().into_owned()])
        .await
        .unwrap()
        .remove(0);
    for (number, (mode, before)) in [
        (CompareMode::Uncommitted, "base"),
        (CompareMode::Unstaged, "staged"),
        (CompareMode::Staged, "base"),
    ]
    .into_iter()
    .enumerate()
    {
        // No object-cache or CAS warming leaks between comparison modes.
        let service = GitReview::default();
        let store = ReviewStore::open(fixture.path().join(format!("store-{number}"))).unwrap();
        assert_eq!(store.storage_usage([]).unwrap().blob_count, 0);
        for round in 0..2 {
            let started = Instant::now();
            let comparison = service.compare(&scope, mode.clone(), None, 0, &store).await;
            println!(
                "REVIEW_DENSE_PERFORMANCE {}",
                serde_json::json!({"mode": mode, "round": round, "coldApplicationCache": round == 0, "files": 1000, "ms": started.elapsed().as_secs_f64() * 1000.0, "error": comparison.as_ref().err()})
            );
            let comparison = comparison
                .expect("dense comparison must complete within the unchanged production budget");
            assert_eq!(comparison.changes.len(), 1000);
            for (file, change) in comparison.changes.iter().enumerate() {
                assert_eq!(change.path, format!("file-{file:04}.txt"));
                assert_eq!(
                    store
                        .read_blob(&change.before.as_ref().unwrap().hash)
                        .unwrap(),
                    format!("{before} {file}\n").as_bytes()
                );
                let after = if matches!(mode, CompareMode::Staged) {
                    "staged"
                } else {
                    "working"
                };
                assert_eq!(
                    store
                        .read_blob(&change.after.as_ref().unwrap().hash)
                        .unwrap(),
                    format!("{after} {file}\n").as_bytes()
                );
            }
            assert_eq!(fs::read(root.join(".git/index")).unwrap(), index);
        }
    }
    for file in 0..1000 {
        assert_eq!(
            fs::read(root.join(format!("file-{file:04}.txt"))).unwrap(),
            format!("working {file}\n").as_bytes()
        );
    }
}

#[tokio::test]
#[ignore = "quantitative 10k-file/1k-commit benchmark; run explicitly with --nocapture"]
async fn review_large_repository_performance() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().join("repo");
    fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q"]);
    // Fast-import makes setup deterministic and avoids 1,000 commit processes.
    let mut stream = String::from("blob\nmark :1\ndata 10\nunchanged\n\n");
    for revision in 0..1000 {
        writeln!(stream, "commit refs/heads/main\ncommitter Benchmark <benchmark@example.invalid> {} +0000\ndata 1\nx", 1_700_000_000 + revision).unwrap();
        if revision == 0 {
            for file in 0..10000 {
                writeln!(stream, "M 100644 :1 unchanged-{file}.txt").unwrap();
            }
        }
        let content = format!("revision {revision}\n");
        writeln!(
            stream,
            "M 100644 inline changed.txt\ndata {}\n{content}",
            content.len()
        )
        .unwrap();
    }
    let mut importer = Command::new("git")
        .current_dir(&root)
        .args(["fast-import", "--quiet"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    importer
        .stdin
        .take()
        .unwrap()
        .write_all(stream.as_bytes())
        .unwrap();
    let output = importer.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    git(&root, &["checkout", "-q", "main"]);
    git(&root, &["branch", "baseline", "HEAD~999"]);
    fs::write(root.join("changed.txt"), "staged change\n").unwrap();
    git(&root, &["add", "--", "changed.txt"]);
    fs::write(root.join("changed.txt"), "working change\n").unwrap();
    let original_index = fs::read(root.join(".git/index")).unwrap();
    let service = GitReview::default();
    let scope = service
        .discover(&[root.to_string_lossy().into_owned()])
        .await
        .unwrap()
        .remove(0);
    let store = ReviewStore::open(fixture.path().join("store")).unwrap();
    let mut results = Vec::new();
    for (label, mode, reference) in [
        ("uncommitted", CompareMode::Uncommitted, None),
        ("unstaged", CompareMode::Unstaged, None),
        ("staged", CompareMode::Staged, None),
        ("committed", CompareMode::Committed, Some("HEAD")),
        ("branch", CompareMode::Branch, Some("baseline")),
    ] {
        let mut samples = Vec::new();
        for _ in 0..5 {
            let started = Instant::now();
            let comparison = service
                .compare(&scope, mode.clone(), reference, 0, &store)
                .await
                .unwrap();
            samples.push(started.elapsed().as_secs_f64() * 1000.0);
            assert_eq!(comparison.changes.len(), 1);
            assert_eq!(comparison.changes[0].path, "changed.txt");
        }
        let first = samples[0];
        samples.sort_by(f64::total_cmp);
        results.push(serde_json::json!({"operation": label, "firstMs": first, "medianMs": samples[2], "maxMs": samples[4], "samples": 5}));
    }
    for skip in [0, 950] {
        let started = Instant::now();
        let commits = service.commits(&scope, skip).await.unwrap();
        assert_eq!(commits.len(), 50);
        results.push(serde_json::json!({"operation": "history", "skip": skip, "ms": started.elapsed().as_secs_f64() * 1000.0}));
    }
    assert_eq!(fs::read(root.join(".git/index")).unwrap(), original_index);
    assert_eq!(
        fs::read(root.join("changed.txt")).unwrap(),
        b"working change\n"
    );
    let cached_blobs = service.blobs.lock().unwrap().len();
    assert!(
        cached_blobs < 20,
        "unchanged tree content must not populate cache"
    );
    println!(
        "REVIEW_PERFORMANCE {}",
        serde_json::json!({"os": std::env::consts::OS, "arch": std::env::consts::ARCH, "files": 10001, "commits": 1000, "cachedBlobs": cached_blobs, "results": results})
    );
}

#[tokio::test]
#[ignore = "quantitative mixed-size files and process peak memory; run explicitly"]
async fn review_mixed_files_performance() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().join("repo");
    fs::create_dir(&root).unwrap();
    git(&root, &["init", "-q"]);
    let content = |file: usize, changed: bool| {
        let size = if file.is_multiple_of(2) {
            2 * 1024 * 1024
        } else {
            4096
        };
        let mut bytes = vec![b'a' + u8::try_from(file % 26).unwrap(); size];
        bytes[..8].copy_from_slice(&u64::try_from(file).unwrap().to_le_bytes());
        bytes[size - 1] = if changed { b'X' } else { b'Y' };
        bytes
    };
    for file in 0..32 {
        fs::write(
            root.join(format!("mixed-{file:02}.bin")),
            content(file, false),
        )
        .unwrap();
    }
    git(&root, &["add", "."]);
    git(
        &root,
        &[
            "-c",
            "user.name=Review Test",
            "-c",
            "user.email=review@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "mixed",
        ],
    );
    for file in 0..32 {
        fs::write(
            root.join(format!("mixed-{file:02}.bin")),
            content(file, true),
        )
        .unwrap();
    }
    let index = fs::read(root.join(".git/index")).unwrap();
    let service = GitReview::default();
    let scope = service
        .discover(&[root.to_string_lossy().into_owned()])
        .await
        .unwrap()
        .remove(0);
    let store = ReviewStore::open(fixture.path().join("store")).unwrap();
    for round in 0..2 {
        let started = Instant::now();
        let result = service
            .compare(&scope, CompareMode::Uncommitted, None, 0, &store)
            .await;
        println!(
            "REVIEW_MIXED_PERFORMANCE {}",
            serde_json::json!({"round": round, "files": 32, "largeFileBytes": 2_097_152, "smallFileBytes": 4096, "ms": started.elapsed().as_secs_f64() * 1000.0, "error": result.as_ref().err()})
        );
        let result = result.expect("mixed-size query must satisfy production budget");
        assert_eq!(result.changes.len(), 32);
        for (file, change) in result.changes.iter().enumerate() {
            assert_eq!(change.path, format!("mixed-{file:02}.bin"));
            assert_eq!(
                store
                    .read_blob(&change.before.as_ref().unwrap().hash)
                    .unwrap(),
                content(file, false)
            );
            assert_eq!(
                store
                    .read_blob(&change.after.as_ref().unwrap().hash)
                    .unwrap(),
                content(file, true)
            );
        }
        assert_eq!(fs::read(root.join(".git/index")).unwrap(), index);
    }
    for file in 0..32 {
        assert_eq!(
            fs::read(root.join(format!("mixed-{file:02}.bin"))).unwrap(),
            content(file, true)
        );
    }
}
