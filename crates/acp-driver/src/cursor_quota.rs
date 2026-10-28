//! macOS native TUI adapter. Sends /usage only after its local command menu
//! advertises it; never passes an initial model prompt or reads credentials.
use driver_api::{DriverError, QuotaSnapshot, QuotaWindow};
use std::{path::Path, sync::Arc};

#[cfg(all(test, unix))]
mod scope_tests;

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn parse(text: &str) -> Option<QuotaSnapshot> {
    let header = regex::Regex::new(r"Usage • ([^\r\n]+?)\s+Resets ([^\r\n]+)").ok()?;
    let capture = header.captures(text)?;
    if !text.contains("Esc to close") {
        return None;
    }
    let percent = |label: &str| -> Option<f64> {
        let pattern = regex::Regex::new(&format!(r"\b(?:{label})\s*:?\s+([0-9]+)% used\b")).ok()?;
        let used = pattern
            .captures(text)?
            .get(1)?
            .as_str()
            .parse::<f64>()
            .ok()?;
        (used <= 100.0).then_some(used)
    };
    let window = |id: &str, scope: &str, used| QuotaWindow {
        id: id.into(),
        scope: Some(scope.into()),
        window_minutes: 0,
        used_percent: Some(used),
        resets_at_unix: None,
    };
    // Cursor's CLI retains the legacy Auto/API names for the desktop's
    // Cursor Models/Other Models pools. Included is not either pool's balance.
    let has_pools = regex::Regex::new(r"\b(?:Auto|API|Cursor Models|Other Models)\s*:?\s+")
        .ok()?
        .is_match(text);
    let windows = if has_pools {
        vec![
            window(
                "cursor-models",
                "Cursor Models",
                percent("Auto|Cursor Models")?,
            ),
            window(
                "cursor-other-models",
                "Other Models",
                percent("API|Other Models")?,
            ),
        ]
    } else {
        // Older clients may only report the aggregate; do not invent a split.
        vec![window(
            "cursor-included",
            "套餐内（终端读数）",
            percent("Included")?,
        )]
    };
    let billing = if text.contains("On-demand usage is off") {
        "按量付费关闭"
    } else {
        "按量付费状态见原生客户端"
    };
    let now = chrono::Utc::now().timestamp_millis();
    Some(QuotaSnapshot {
        session_usage: None,
        windows,
        // The TUI gives month/day only: preserve display text, never invent year/time.
        plan_type: Some(format!(
            "{} · 重置 {}（原生日期）· {billing}",
            capture[1].trim(),
            capture[2].trim()
        )),
        collected_at_unix_ms: now,
        stale_after_unix_ms: now + 60_000,
        source: "native_tui".into(),
        confidence: "provider_reported".into(),
        parser_version: "cursor-tui-usage-v2".into(),
        context: None,
    })
}

#[cfg(not(target_os = "macos"))]
// Same async signature as the macOS reader, which the caller awaits.
#[allow(clippy::unused_async)]
pub(super) async fn read(
    _: &Path,
    _: Arc<tempfile::TempDir>,
    _: Arc<tokio::sync::Mutex<()>>,
) -> Result<QuotaSnapshot, DriverError> {
    Err(DriverError::NotConfigured(
        "Cursor native quota currently requires macOS".into(),
    ))
}

#[cfg(target_os = "macos")]
pub(super) async fn read(
    executable: &Path,
    directory: Arc<tempfile::TempDir>,
    admission: Arc<tokio::sync::Mutex<()>>,
) -> Result<QuotaSnapshot, DriverError> {
    let mut command = tokio::process::Command::new(executable);
    command
        // Reading /usage must not start Cursor's background installer. This
        // short-lived TUI otherwise can exit with a download and lock in flight.
        .arg("--disable-auto-update")
        // Trust applies only to this newly created, empty probe directory.
        .args(["--mode", "ask", "--trust"])
        .current_dir(directory.path())
        .env_clear()
        .env("TERM", "xterm-256color")
        .env("COLUMNS", "120")
        .env("LINES", "40");
    for key in ["HOME", "PATH", "LANG", "TZ", "TMPDIR"] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    agent_runtime::apply_proxy_environment(&mut command);
    read_owned(command, directory, admission).await
}

#[cfg(any(target_os = "macos", all(test, unix)))]
fn unavailable() -> DriverError {
    DriverError::Protocol("Cursor native usage unavailable or terminal layout changed".into())
}

#[cfg(any(target_os = "macos", all(test, unix)))]
async fn read_owned(
    command: tokio::process::Command,
    directory: Arc<tempfile::TempDir>,
    admission: Arc<tokio::sync::Mutex<()>>,
) -> Result<QuotaSnapshot, DriverError> {
    let permit = tokio::time::timeout(std::time::Duration::from_secs(45), admission.lock_owned())
        .await
        .map_err(|_| {
            DriverError::CapabilityUnavailable("上一项账户额度查询仍在结束，请稍后重试".into())
        })?;
    let (mut reply, result) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let _permit = permit;
        let _directory = directory;
        if reply.is_closed() {
            return;
        }
        let Ok((mut child, input, output)) =
            agent_runtime::OwnedChild::spawn_terminal(command, 120, 40)
        else {
            let _ = reply.send(Err(unavailable()));
            return;
        };
        let query = async {
            // A descendant may retain the terminal after its leader exits.
            // Observe the actual leader concurrently so its original scope is
            // retired and terminal EOF can arrive, rather than waiting forever.
            let (status, snapshot) = tokio::try_join!(
                async { child.wait().await.map_err(|_| unavailable()) },
                read_terminal(
                    tokio::fs::File::from_std(input),
                    tokio::fs::File::from_std(output)
                ),
            )?;
            if !status.success() {
                return Err(unavailable());
            }
            Ok(snapshot)
        };
        let outcome = tokio::select! {
            biased;
            () = reply.closed() => None,
            result = query => Some(result),
        };
        while child.terminate().await.is_err() {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
        if let Some(outcome) = outcome {
            let _ = reply.send(outcome);
        }
    });
    result.await.map_err(|_| unavailable())?
}

#[cfg(any(target_os = "macos", all(test, unix)))]
async fn read_terminal(
    mut input: tokio::fs::File,
    mut output: tokio::fs::File,
) -> Result<QuotaSnapshot, DriverError> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let ansi = regex::Regex::new(r"\x1b\[[0-?]*[ -/]*[@-~]|\x1b\][^\x07]*\x07")
        .map_err(|_| unavailable())?;
    let mut raw = Vec::new();
    let mut buffer = [0_u8; 8192];
    let mut state = 0;
    let mut result = None;
    loop {
        let count = match output.read(&mut buffer).await {
            Ok(count) => count,
            // Unix PTYs may report EIO on slave hangup rather than zero.
            Err(error) if error.raw_os_error() == Some(5) => 0,
            Err(_) => return Err(unavailable()),
        };
        if count == 0 {
            break;
        }
        raw.extend_from_slice(&buffer[..count]);
        if raw.len() > 512 * 1024 {
            return Err(unavailable());
        }
        let decoded = String::from_utf8_lossy(&raw);
        let screen = ansi.replace_all(&decoded, "");
        let send = match state {
            0 if screen.contains("Workspace Trust Required") => {
                state = 1;
                Some("a\r")
            }
            0 | 1 if screen.contains("Plan, search, build anything") => {
                state = 2;
                Some("/usage")
            }
            2 if screen.contains("Show plan and on-demand usage") => {
                state = 3;
                Some("\r")
            }
            3 if parse(&screen).is_some() => {
                result = parse(&screen);
                state = 4;
                Some("\x1b")
            }
            4 if screen.contains("Plan, search, build anything") => {
                state = 5;
                Some("/quit")
            }
            5 if screen.contains("/quit") && screen.contains("Exit") => {
                state = 6;
                Some("\r")
            }
            _ => None,
        };
        if let Some(text) = send {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            #[cfg(test)]
            eprintln!("Cursor quota state: {state}");
            input
                .write_all(text.as_bytes())
                .await
                .map_err(|_| unavailable())?;
            input.flush().await.map_err(|_| unavailable())?;
            raw.clear();
        }
    }
    if state != 6 {
        return Err(unavailable());
    }
    result.ok_or_else(unavailable)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_native_date_and_rounded_informational_usage() {
        let text = "Usage • Pro      Resets Oct 4\nIncluded     15% used\nOn-demand usage is off\nEsc to close";
        let quota = parse(text).unwrap();
        assert_eq!(quota.windows[0].used_percent, Some(15.0));
        assert_eq!(quota.windows[0].resets_at_unix, None);
        assert!(quota.windows[0].scope.is_some());
        assert!(parse(&text.replace("15%", "101%")).is_none());
        assert!(parse("Loading usage data...").is_none());
    }
    #[test]
    fn separates_native_model_pools_without_using_the_aggregate() {
        for (first, other) in [
            ("Auto", "API"),
            ("Auto:", "API:"),
            ("Cursor Models", "Other Models"),
        ] {
            let text = format!(
                "Usage • Pro      Resets Oct 4\nIncluded 65% used\n{first} 20% used\n{other} 100% used\nOn-demand usage is off\nEsc to close"
            );
            let quota = parse(&text).unwrap();
            assert_eq!(quota.windows.len(), 2);
            assert_eq!(quota.windows[0].id, "cursor-models");
            assert_eq!(quota.windows[0].scope.as_deref(), Some("Cursor Models"));
            assert_eq!(quota.windows[0].used_percent, Some(20.0));
            assert_eq!(quota.windows[1].id, "cursor-other-models");
            assert_eq!(quota.windows[1].scope.as_deref(), Some("Other Models"));
            assert_eq!(quota.windows[1].used_percent, Some(100.0));
            assert!(quota.windows.iter().all(|w| w.resets_at_unix.is_none()));
            assert!(parse(&text.replace("100%", "101%")).is_none());
            assert!(parse(&text.replace(&format!("{other} 100% used"), "Loading...")).is_none());
        }
    }
    #[tokio::test]
    #[ignore = "requires authenticated Cursor on macOS; native command only"]
    async fn real_native_quota() {
        let executable = std::env::var("CURSOR_QUOTA_EXECUTABLE").unwrap();
        let directory = tempfile::tempdir().unwrap();
        let snapshot = tokio::time::timeout(
            std::time::Duration::from_secs(27),
            read(
                Path::new(&executable),
                Arc::new(directory),
                Arc::new(tokio::sync::Mutex::new(())),
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(!snapshot.windows.is_empty());
    }
}
