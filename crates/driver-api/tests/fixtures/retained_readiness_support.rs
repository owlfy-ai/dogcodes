//! Shared private-process test support, included only by driver unit tests.
use std::{path::PathBuf, process::Command, time::Duration};

pub struct Fixture {
    pub root: tempfile::TempDir,
    pub executable: PathBuf,
}
impl Fixture {
    pub fn new(mode: &str) -> Self {
        let root = tempfile::tempdir().unwrap();
        let executable = root.path().join(if cfg!(windows) {
            "runtime.exe"
        } else {
            "runtime"
        });
        let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../driver-api/tests/fixtures/retained_readiness.rs");
        let mut command = Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()));
        command
            .args(["--edition=2024", "--crate-name", "readiness_fixture"])
            .arg(source)
            .arg("-o")
            .arg(&executable);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "fixture compilation failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        std::fs::write(root.path().join("mode"), mode).unwrap();
        Self { root, executable }
    }
    pub fn starts(&self) -> Vec<u32> {
        std::fs::read_to_string(self.root.path().join("starts"))
            .unwrap_or_default()
            .lines()
            .map(|line| line.parse().unwrap())
            .collect()
    }
    pub fn requests(&self, name: &str) -> usize {
        std::fs::read_to_string(self.root.path().join("requests"))
            .unwrap_or_default()
            .lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .filter(|value| {
                value["method"] == name
                    || value["type"] == name
                    || value.pointer("/request/subtype").is_some_and(|v| v == name)
            })
            .count()
    }
}
pub async fn wait_for(mut ready: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(12), async {
        while !ready() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}
