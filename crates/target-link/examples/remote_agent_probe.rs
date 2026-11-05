//! Drives a real ACP agent on a real server through the ordinary driver, the way the
//! app will. Run it yourself: it asks for any password on the terminal, with echo off.
//!
//! ```sh
//! node scripts/prepare-shims.mjs
//! cargo run -p target-link --example remote_agent_probe -- root@host
//! cargo run -p target-link --example remote_agent_probe -- root@host --prompt "Reply with OK"
//! ```
//!
//! Without `--prompt` no model is called. Prompts count against the agent's account.
use agent_protocol::{SshAuth, SshConnection};
use agent_runtime::{Pipe, SharedTarget, TargetCommand, TargetExecutable};
use driver_api::{AgentDriver, DriverEvent, PromptRequest, SessionRequest};
use futures::StreamExt;
use std::{io::Write, path::PathBuf, sync::Arc, time::Duration};
use target_link::{
    AskpassPrompt, ConnectObserver, ConnectRequest, ConnectStep, PromptKind, ShimCatalog,
};
use tokio::io::AsyncReadExt;

struct Terminal;

fn read_line_hidden(hidden: bool) -> Option<String> {
    let run_stty = |flag: &str| {
        let _ = std::process::Command::new("stty")
            .arg(flag)
            .stdin(std::process::Stdio::inherit())
            .status();
    };
    if hidden {
        run_stty("-echo");
    }
    let mut line = String::new();
    let result = std::io::stdin().read_line(&mut line);
    if hidden {
        run_stty("echo");
        eprintln!();
    }
    result.ok()?;
    Some(line.trim_end_matches(['\r', '\n']).to_owned())
}

#[async_trait::async_trait]
impl ConnectObserver for Terminal {
    fn step(&self, step: ConnectStep) {
        if !matches!(step, ConnectStep::Deploying { sent, .. } if sent > 0) {
            eprintln!("[step] {step:?}");
        }
    }
    fn log(&self, line: &str) {
        eprintln!("[ssh] {line}");
    }
    async fn prompt(&self, prompt: AskpassPrompt) -> Option<String> {
        eprint!("{} ", prompt.text);
        let _ = std::io::stderr().flush();
        let hidden = prompt.kind != PromptKind::HostKey;
        tokio::task::spawn_blocking(move || read_line_hidden(hidden))
            .await
            .ok()?
    }
}

async fn capture(target: &SharedTarget, script: &str) -> String {
    let mut command = TargetCommand::new(target.clone(), "/bin/sh");
    command
        .args(["-c", script])
        .inherit("PATH")
        .inherit("HOME")
        .stdio(Pipe::Null, Pipe::Piped, Pipe::Null);
    let Ok(mut child) = command.spawn() else {
        return String::new();
    };
    let mut output = String::new();
    if let Some(mut stdout) = child.stdout.take() {
        let _ = stdout.read_to_string(&mut output).await;
    }
    let _ = child.wait().await;
    output.trim().to_owned()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if target_link::askpass::run_if_requested() {
        return Ok(());
    }
    let mut args = std::env::args().skip(1);
    let destination = args
        .next()
        .ok_or("usage: remote_agent_probe user@host [--prompt text] [--agent name]")?;
    let (mut prompt, mut agent) = (None, "kimi".to_owned());
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--prompt" => prompt = args.next(),
            "--agent" => agent = args.next().ok_or("--agent needs a name")?,
            other => return Err(format!("unknown argument {other}").into()),
        }
    }
    let (user, host) = destination
        .split_once('@')
        .map_or((None, destination.as_str()), |(user, host)| {
            (Some(user.to_owned()), host)
        });
    let shims = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../apps/desktop/src-tauri/resources/shim");
    let link = target_link::connect(
        ConnectRequest {
            connection: SshConnection {
                destination: host.to_owned(),
                port: None,
                user,
                auth: SshAuth::System,
                identity_file: None,
                install_directory: None,
                proxy: None,
            },
            program: target_link::find_ssh().await?,
            askpass_program: std::env::current_exe()?,
            shims: ShimCatalog::new(vec![shims]),
            config_file: None,
        },
        Arc::new(Terminal),
    )
    .await?;
    let info = link.info();
    println!(
        "connected: {} {} helper {}",
        info.os, info.arch, info.version
    );
    let target = link.target();
    println!("login PATH present: {}", target.inherited("PATH").is_some());

    let found = capture(
        &target,
        &format!("command -v {agent} || ls \"$HOME/.{agent}-code/bin/{agent}\" 2>/dev/null"),
    )
    .await;
    if found.is_empty() {
        return Err(format!("{agent} was not found on the server").into());
    }
    println!("{agent}: {found}");
    println!(
        "version: {}",
        capture(&target, &format!("{found} --version 2>&1 | head -1")).await
    );

    let started = std::time::Instant::now();
    let driver = acp_driver::AcpDriver::connect_ready(
        &agent,
        &agent_domain::InstallationId::new().to_string(),
        acp_driver::AcpLaunch {
            executable: TargetExecutable::new(target.clone(), &found),
            args: vec!["acp".into()],
        },
        None,
    )
    .await?;
    println!(
        "ACP ready over SSH in {:?}, agent version {:?}",
        started.elapsed(),
        driver.native_agent_version()
    );
    let models = driver.model_catalog().await?;
    println!(
        "models: {}",
        models
            .models
            .iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );

    if let Some(text) = prompt {
        let home = target
            .inherited("HOME")
            .and_then(|home| home.into_string().ok())
            .ok_or("HOME is not set on the server")?;
        let handle = driver
            .create_session(SessionRequest {
                working_directory: home.clone(),
                additional_directories: vec![],
                app_mcps: vec![],
                permission_mode: None,
            })
            .await?;
        println!("session: {}", handle.native_session_id);
        let mut events = driver
            .prompt(
                &handle,
                PromptRequest {
                    attachments: vec![],
                    permission_mode: None,
                    additional_directories: vec![],
                    prompt: text,
                    working_directory: home,
                    context: vec![],
                },
            )
            .await?;
        let mut reasoning = 0_u32;
        let reasoning_chunks = &mut reasoning;
        let finished = tokio::time::timeout(Duration::from_mins(3), async {
            while let Some(event) = events.next().await {
                match event? {
                    DriverEvent::TextDelta { text, .. } => {
                        print!("{text}");
                        let _ = std::io::stdout().flush();
                    }
                    // ACP delivers replies as display events rather than text deltas.
                    // Reasoning is only counted: it is not ours to print or keep.
                    DriverEvent::Presentation { data, .. } => {
                        let delta = data["textDelta"]
                            .as_str()
                            .or_else(|| data["delta"].as_str())
                            .unwrap_or_default();
                        let kind = data["type"].as_str().unwrap_or("unknown").to_owned();
                        if kind == "thought" {
                            *reasoning_chunks += 1;
                        } else if !delta.is_empty() {
                            print!("{delta}");
                            let _ = std::io::stdout().flush();
                        } else {
                            eprintln!("[presentation] {kind}");
                        }
                    }
                    DriverEvent::TurnCompleted { .. } => {
                        println!("\nturn completed");
                        return Ok::<_, driver_api::DriverError>(true);
                    }
                    DriverEvent::TurnFailed { reason } => {
                        println!("\nturn failed: {reason}");
                        return Ok(false);
                    }
                    other => eprintln!("[event] {}", kind(&other)),
                }
            }
            Ok(false)
        })
        .await??;
        println!("reasoning chunks: {reasoning}");
        println!("result: {}", if finished { "ok" } else { "not completed" });
    }
    drop(driver);
    let mut closed = link.closed();
    drop(link);
    let _ = tokio::time::timeout(Duration::from_secs(5), closed.wait_for(Option::is_some)).await;
    println!("disconnected");
    Ok(())
}

fn kind(event: &DriverEvent) -> &'static str {
    match event {
        DriverEvent::FirstOutput => "first_output",
        DriverEvent::UsageRecorded { .. } => "usage",
        DriverEvent::Presentation { .. } => "presentation",
        DriverEvent::SessionReady { .. } => "session_ready",
        DriverEvent::TurnStarted { .. } => "turn_started",
        DriverEvent::ToolStarted { .. } => "tool_started",
        DriverEvent::ToolCompleted { .. } => "tool_completed",
        DriverEvent::ApprovalRequested { .. } => "approval_requested",
        _ => "other",
    }
}
