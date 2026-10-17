//! Exercise the actual CLI only against owned, synthetic files; never launches a model/runtime.
use serde_json::{Value, json};
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn run(command: &str, input: &Path, output: Option<&Path>) -> Output {
    let mut process = Command::new(env!("CARGO_BIN_EXE_codex-app-server-probe"));
    process.arg(command).arg(input);
    if let Some(path) = output {
        process.arg(path);
    }
    process.output().unwrap()
}

#[test]
fn exports_and_checks_sanitized_jsonl_without_touching_input() {
    let directory = tempfile::tempdir().unwrap();
    let input = directory.path().join("private-input.jsonl");
    let output = directory.path().join("safe.jsonl");
    let raw = b"{\"method\":\"turn/completed\",\"params\":{\"threadId\":\"SECRET_SENTINEL\",\"turn\":{\"id\":\"SECRET_SENTINEL\",\"status\":\"completed\",\"future\":\"SECRET_SENTINEL\"}}}\n{\"method\":\"SECRET_SENTINEL\",\"params\":{\"token\":123}}\n";
    fs::write(&input, raw).unwrap();
    let result = run("sanitize-fixture", &input, Some(&output));
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let report: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report["frameCount"], 2);
    assert_eq!(fs::read(&input).unwrap(), raw);
    let safe = fs::read_to_string(&output).unwrap();
    assert!(!safe.contains("SECRET_SENTINEL"));
    assert!(!safe.contains("future"));
    assert!(
        run("check-sanitized-fixture", &output, None)
            .status
            .success()
    );
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 2);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&output).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[test]
fn malformed_or_oversized_imports_leave_no_partial_output_or_sensitive_errors() {
    let directory = tempfile::tempdir().unwrap();
    let input = directory.path().join("SECRET_SENTINEL.jsonl");
    let output = directory.path().join("safe.jsonl");
    for raw in [
        b"{}\n{\"password\":\"SECRET_SENTINEL".to_vec(),
        vec![b' '; 1024 * 1024 + 1],
        b"{}\n".repeat(257),
    ] {
        fs::write(&input, &raw).unwrap();
        let result = run("sanitize-fixture", &input, Some(&output));
        assert!(!result.status.success());
        assert!(!output.exists());
        assert_eq!(fs::read(&input).unwrap(), raw);
        assert!(!String::from_utf8_lossy(&result.stderr).contains("SECRET_SENTINEL"));
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }
}

#[test]
fn never_overwrites_existing_files_or_symlinks() {
    let directory = tempfile::tempdir().unwrap();
    let input = directory.path().join("input.jsonl");
    let output = directory.path().join("existing.jsonl");
    fs::write(&input, b"{}\n").unwrap();
    fs::write(&output, b"keep").unwrap();
    assert!(
        !run("sanitize-fixture", &input, Some(&output))
            .status
            .success()
    );
    assert_eq!(fs::read(&output).unwrap(), b"keep");
    #[cfg(unix)]
    {
        let link = directory.path().join("link.jsonl");
        std::os::unix::fs::symlink(&output, &link).unwrap();
        assert!(
            !run("sanitize-fixture", &input, Some(&link))
                .status
                .success()
        );
        assert!(
            !run(
                "sanitize-fixture",
                &link,
                Some(&directory.path().join("new.jsonl"))
            )
            .status
            .success()
        );
        assert_eq!(fs::read(&output).unwrap(), b"keep");
    }
}

#[test]
fn secondary_scan_rejects_injected_metadata_payloads_and_duplicate_keys() {
    let directory = tempfile::tempdir().unwrap();
    let input = directory.path().join("input.jsonl");
    let output = directory.path().join("safe.jsonl");
    fs::write(
        &input,
        b"{\"method\":\"turn/completed\",\"params\":{\"turn\":{\"status\":\"completed\"}}}\n",
    )
    .unwrap();
    assert!(
        run("sanitize-fixture", &input, Some(&output))
            .status
            .success()
    );
    let safe: Value = serde_json::from_slice(&fs::read(&output).unwrap()).unwrap();
    for pointer in [
        "/parserVersion",
        "/driverVersion",
        "/sourceKind",
        "/diagnostic/payload/params/turn/status",
        "/nativeVersion",
        "/diagnostic/audit/droppedFields",
    ] {
        let mut changed = safe.clone();
        *changed.pointer_mut(pointer).unwrap() = json!("SECRET_SENTINEL");
        fs::write(&output, serde_json::to_vec(&changed).unwrap()).unwrap();
        let result = run("check-sanitized-fixture", &output, None);
        assert!(!result.status.success());
        assert!(!String::from_utf8_lossy(&result.stderr).contains("SECRET_SENTINEL"));
    }
    let canonical = serde_json::to_string(&safe).unwrap();
    let duplicate = format!(
        "{{\"parserVersion\":\"SECRET_SENTINEL\",{}",
        &canonical[1..]
    );
    fs::write(&output, duplicate).unwrap();
    assert!(
        !run("check-sanitized-fixture", &output, None)
            .status
            .success()
    );
}
