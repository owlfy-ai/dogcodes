//! Bounded service-owned diagnostics survive Desktop exits and startup failures.
use redaction::{DeterministicRedactor, SecretRedactor};
use serde_json::{Value, json};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

const MAX_LOG_BYTES: u64 = 256 * 1024;
const MAX_RECORD_BYTES: usize = 16 * 1024;
const REPORT_TAIL_BYTES: u64 = 32 * 1024;
const RETAINED_FILES: usize = 8;
static SERVICE_LOG: OnceLock<ServiceLog> = OnceLock::new();

pub struct DiagnosticDirectory(pub PathBuf);
struct ServiceLog {
    file: Mutex<File>,
    path: PathBuf,
    _lease: File,
}

fn private_file(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

fn log_files(directory: &Path) -> Vec<PathBuf> {
    let mut paths = std::fs::read_dir(directory)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                n.starts_with("core-")
                    && matches!(
                        path.extension().and_then(|extension| extension.to_str()),
                        Some("log" | "previous")
                    )
            })
        })
        .collect::<Vec<_>>();
    paths.sort_by_key(|path| std::cmp::Reverse(path.metadata().and_then(|m| m.modified()).ok()));
    paths
}

fn prune(directory: &Path) {
    for path in log_files(directory).into_iter().skip(RETAINED_FILES) {
        // Lease locks protect live writers without locking the data bytes:
        // Windows shared byte locks also prohibit the writer's own writes.
        let lease_path = path.with_extension("lock");
        let Ok(lease) = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lease_path)
        else {
            continue;
        };
        if lease.try_lock().is_err() {
            continue;
        }
        // Respect data-file locks held by older versions as well.
        if let Ok(file) = OpenOptions::new().read(true).write(true).open(&path)
            && file.try_lock().is_ok()
        {
            drop(file);
            if std::fs::remove_file(&path).is_ok()
                && !path.with_extension("log").exists()
                && !path.with_extension("previous").exists()
            {
                drop(lease);
                let _ = std::fs::remove_file(lease_path);
            }
        }
    }
}

impl ServiceLog {
    fn open(directory: &Path) -> std::io::Result<Self> {
        let directory = directory.join("diagnostics");
        std::fs::create_dir_all(&directory)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
        }
        let path = directory.join(format!(
            "core-{}-{}.log",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let lease_path = path.with_extension("lock");
        let lease = private_file(&lease_path)?;
        lease.try_lock()?;
        let file = match private_file(&path) {
            Ok(file) => file,
            Err(error) => {
                drop(lease);
                let _ = std::fs::remove_file(lease_path);
                return Err(error);
            }
        };
        prune(&directory);
        Ok(Self {
            file: Mutex::new(file),
            path,
            _lease: lease,
        })
    }

    fn record(&self, event: &str, detail: &str) -> std::io::Result<()> {
        let mut detail = DeterministicRedactor.redact(detail).text;
        if detail.len() > MAX_RECORD_BYTES {
            let mut end = MAX_RECORD_BYTES;
            while !detail.is_char_boundary(end) {
                end -= 1;
            }
            detail.truncate(end);
            detail.push_str(" [truncated]");
        }
        let mut bytes = serde_json::to_vec(
            &json!({"at":chrono::Utc::now(),"pid":std::process::id(),"event":event,"detail":detail}),
        )?;
        bytes.push(b'\n');
        let mut file = self
            .file
            .lock()
            .map_err(|_| std::io::Error::other("diagnostic lock unavailable"))?;
        if file.metadata()?.len() + bytes.len() as u64 > MAX_LOG_BYTES {
            let previous = self.path.with_extension("previous");
            let _ = std::fs::remove_file(&previous);
            let mut backup = private_file(&previous)?;
            file.seek(SeekFrom::Start(0))?;
            std::io::copy(&mut *file, &mut backup)?;
            file.set_len(0)?;
            prune(self.path.parent().expect("log directory"));
        }
        file.seek(SeekFrom::End(0))?;
        file.write_all(&bytes)?;
        file.flush()
    }
}

pub fn record(event: &str, detail: &str) {
    if let Some(log) = SERVICE_LOG.get() {
        let _ = log.record(event, detail);
    }
}

pub fn install(directory: &Path) {
    let Ok(log) = ServiceLog::open(directory) else {
        return;
    };
    if SERVICE_LOG.set(log).is_err() {
        return;
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        record(
            "panic",
            &format!("{info}\n{}", std::backtrace::Backtrace::force_capture()),
        );
        previous(info);
    }));
    record("started", env!("CARGO_PKG_VERSION"));
}

/// Export only a bounded tail and apply the same rules again to older files.
pub fn report(directory: &Path) -> Value {
    let entries = log_files(&directory.join("diagnostics"))
        .into_iter()
        .take(2)
        .filter_map(|path| {
            let mut file = File::open(path).ok()?;
            let length = file.metadata().ok()?.len();
            let offset = length.saturating_sub(REPORT_TAIL_BYTES);
            file.seek(SeekFrom::Start(offset)).ok()?;
            let mut bytes = Vec::new();
            file.take(REPORT_TAIL_BYTES).read_to_end(&mut bytes).ok()?;
            let text = String::from_utf8_lossy(&bytes);
            // Discard an incomplete first JSON record when reading a tail.
            let text = if offset > 0 {
                text.split_once('\n').map_or("", |(_, rest)| rest)
            } else {
                &text
            };
            Some(DeterministicRedactor.redact(text).text)
        })
        .collect::<Vec<_>>();
    json!({"redactionRulesetVersion":redaction::REDACTION_RULESET_VERSION,"logs":entries})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostics_redact_secrets_rotate_and_export_bounded_tails() {
        let directory = tempfile::tempdir().unwrap();
        let log = ServiceLog::open(directory.path()).unwrap();
        log.record(
            "failed",
            "API_KEY=private-credential user@example.com /Users/alice/project",
        )
        .unwrap();
        let text = std::fs::read_to_string(&log.path).unwrap();
        assert!(!text.contains("private-credential"));
        assert!(!text.contains("alice"));
        assert!(!text.contains("user@example.com"));
        for _ in 0..40 {
            log.record("phase", &"中文 diagnostic ".repeat(1400))
                .unwrap();
        }
        assert!(log.path.metadata().unwrap().len() <= MAX_LOG_BYTES);
        assert!(log.path.with_extension("previous").is_file());
        let report = report(directory.path());
        assert_eq!(report["logs"].as_array().unwrap().len(), 2);
        for value in report["logs"].as_array().unwrap() {
            for line in value.as_str().unwrap().lines() {
                assert!(serde_json::from_str::<Value>(line).is_ok());
            }
        }
        assert!(serde_json::to_vec(&report).unwrap().len() < 100_000);
    }

    #[test]
    fn retention_preserves_active_logs_and_other_files() {
        let directory = tempfile::tempdir().unwrap();
        let active = ServiceLog::open(directory.path()).unwrap();
        active.record("started", "keep").unwrap();
        let unrelated = directory.path().join("diagnostics/other.txt");
        std::fs::write(&unrelated, "keep").unwrap();
        for _ in 0..12 {
            drop(ServiceLog::open(directory.path()).unwrap());
        }
        assert!(active.path.exists());
        assert!(unrelated.exists());
        assert!(log_files(&directory.path().join("diagnostics")).len() <= RETAINED_FILES + 1);
        let leases = std::fs::read_dir(directory.path().join("diagnostics"))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "lock")
            })
            .count();
        assert!(leases <= RETAINED_FILES + 1);
    }

    #[test]
    fn missing_diagnostics_do_not_block_report_export() {
        let directory = tempfile::tempdir().unwrap();
        assert_eq!(report(directory.path())["logs"], json!([]));
    }

    #[test]
    fn panic_record_survives_child_exit() {
        const CHILD_DIRECTORY: &str = "DOGCODES_TEST_DIAGNOSTIC_PANIC_DIR";
        if let Some(directory) = std::env::var_os(CHILD_DIRECTORY) {
            install(Path::new(&directory));
            panic!("diagnostic fixture API_KEY=private-panic-credential");
        }
        let directory = tempfile::tempdir().unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "service_diagnostics::tests::panic_record_survives_child_exit",
                "--nocapture",
            ])
            .env(CHILD_DIRECTORY, directory.path())
            .output()
            .unwrap();
        assert!(!output.status.success());
        let text = report(directory.path()).to_string();
        assert!(text.contains("panic"));
        assert!(!text.contains("private-panic-credential"));
    }
}
