//! Metadata-only initialization benchmark. Never sends a model prompt.
use acp_driver::{AcpDriver, AcpLaunch};
use antigravity_driver::AntigravityDriver;
use claude_driver::ClaudeDriver;
use codex_driver::CodexDriver;
use driver_api::{AgentDriver, HistoryMcp, SessionRequest, SessionResumeCapability};
use serde_json::{Value, json};
use std::{
    io::{BufRead, Write},
    path::PathBuf,
    time::{Duration, Instant},
};
use zcode_driver::{ZcodeDriver, ZcodeLaunch};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if antigravity_driver::approval_bridge::run_hook_if_requested() {
        return Ok(());
    }
    if std::env::args().any(|arg| arg == "--benchmark-mcp") {
        for line in std::io::stdin().lock().lines() {
            let request: Value = serde_json::from_str(&line?)?;
            let Some(id) = request.get("id") else {
                continue;
            };
            let result = match request["method"].as_str().unwrap_or("") {
                "initialize" => {
                    json!({"protocolVersion":request["params"]["protocolVersion"],"capabilities":{"tools":{}},"serverInfo":{"name":"initialization-benchmark","version":"1"}})
                }
                "tools/list" => json!({"tools":[]}),
                "resources/list" => json!({"resources":[]}),
                "prompts/list" => json!({"prompts":[]}),
                "ping" => json!({}),
                _ => {
                    println!(
                        "{}",
                        json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Method not found"}})
                    );
                    std::io::stdout().flush()?;
                    continue;
                }
            };
            println!("{}", json!({"jsonrpc":"2.0","id":id,"result":result}));
            std::io::stdout().flush()?;
        }
        return Ok(());
    }
    tokio::runtime::Runtime::new()?.block_on(run())
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .ok_or("provide an installation metadata JSON file")?;
    let provider = args.next().ok_or("provide one provider ID")?;
    let rows: Vec<Value> = serde_json::from_slice(&std::fs::read(path)?)?;
    let row = rows
        .iter()
        .find(|row| row["providerId"] == provider)
        .ok_or("installation not found")?;
    let executable = PathBuf::from(row["executable"].as_str().ok_or("executable missing")?);
    let id = row["installationId"]
        .as_str()
        .ok_or("installation ID missing")?;
    let version = row["version"].as_str().map(str::to_owned);
    let directory = tempfile::tempdir()?;
    let request = SessionRequest {
        working_directory: directory.path().to_string_lossy().into_owned(),
        additional_directories: vec![],
        permission_mode: None,
        history_mcp: Some(HistoryMcp {
            name: "initialization-benchmark".into(),
            command: std::env::current_exe()?.to_string_lossy().into_owned(),
            args: vec!["--benchmark-mcp".into()],
            directory: directory.path().to_string_lossy().into_owned(),
        }),
    };
    let start = Instant::now();
    let driver: Box<dyn AgentDriver> = match provider.as_str() {
        "cursor" | "grok" | "kimi" | "codebuddy" => {
            let native_args: Vec<String> = match provider.as_str() {
                "cursor" => vec!["--disable-auto-update", "acp"],
                "grok" => vec!["agent", "stdio"],
                "codebuddy" => vec!["--acp"],
                _ => vec!["acp"],
            }
            .into_iter()
            .map(str::to_owned)
            .collect();
            let launch = AcpLaunch {
                executable,
                args: native_args,
            };
            if provider == "cursor" || std::env::var_os("DOGCODES_BENCH_RETAIN_PROBE").is_some() {
                Box::new(AcpDriver::connect_ready(&provider, id, launch, version).await?)
            } else {
                let report = AcpDriver::probe_ready(&launch).await?;
                Box::new(AcpDriver::new(
                    &provider,
                    id,
                    launch,
                    report.native_version.or(version),
                    report.resume_capability,
                )?)
            }
        }
        "codex" => {
            CodexDriver::probe_ready(&executable).await?;
            Box::new(CodexDriver::new(
                &executable,
                id.to_owned(),
                version.unwrap_or_default(),
            )?)
        }
        "claude-code" => {
            ClaudeDriver::probe_ready(&executable).await?;
            Box::new(ClaudeDriver::new(
                &provider,
                id,
                executable,
                version,
                SessionResumeCapability::Supported,
            )?)
        }
        "antigravity" => {
            AntigravityDriver::probe_ready(&executable).await?;
            Box::new(AntigravityDriver::new(
                &provider,
                id,
                executable,
                version,
                SessionResumeCapability::Supported,
            )?)
        }
        "zcode" => {
            let launch = ZcodeLaunch {
                cli_script: PathBuf::from(
                    executable
                        .to_string_lossy()
                        .replace("Contents/MacOS/ZCode", "Contents/Resources/glm/zcode.cjs"),
                ),
                app_binary: executable,
            };
            let resume = zcode_driver::verified_resume_capability(version.as_deref());
            if std::env::var_os("DOGCODES_BENCH_RETAIN_PROBE").is_some() {
                Box::new(ZcodeDriver::connect_ready(&provider, id, launch, version, resume).await?)
            } else {
                ZcodeDriver::probe_ready(&launch).await?;
                Box::new(ZcodeDriver::new(&provider, id, launch, version, resume)?)
            }
        }
        _ => return Err("unsupported provider".into()),
    };
    println!(
        "{}",
        json!({"provider":provider,"stage":"detection","durationMs":start.elapsed().as_secs_f64()*1000.0})
    );
    let mut handles = vec![];
    let mut failed = false;
    for index in 1..=2 {
        let start = Instant::now();
        let handle = tokio::time::timeout(
            Duration::from_mins(1),
            driver.create_selected_session(request.clone(), None, None),
        )
        .await;
        match handle {
            Ok(Ok(handle)) => {
                println!(
                    "{}",
                    json!({"provider":provider,"stage":format!("new_session_{index}"),"durationMs":start.elapsed().as_secs_f64()*1000.0})
                );
                handles.push(handle);
            }
            Ok(Err(error)) => {
                println!(
                    "{}",
                    json!({"provider":provider,"stage":format!("new_session_{index}"),"error":error.to_string(),"durationMs":start.elapsed().as_secs_f64()*1000.0})
                );
                failed = true;
                break;
            }
            Err(_) => {
                println!(
                    "{}",
                    json!({"provider":provider,"stage":format!("new_session_{index}"),"error":"timeout"})
                );
                failed = true;
                break;
            }
        }
    }
    for handle in handles {
        if !matches!(
            tokio::time::timeout(Duration::from_secs(15), driver.shutdown(handle)).await,
            Ok(Ok(()))
        ) {
            failed = true;
            eprintln!("session cleanup failed");
        }
    }
    if failed {
        return Err("initialization benchmark failed".into());
    }
    Ok(())
}
