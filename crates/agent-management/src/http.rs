//! Requests to public update feeds and account endpoints.
use std::{
    io,
    process::{Output, Stdio},
    time::Duration,
};
use tokio::{io::AsyncWriteExt, process::Command};

const RESPONSE_LIMIT: &str = "1048576";

/// A request through this machine's `curl`. A token travels on stdin, so it never shows
/// in the process list. The config line it goes into is quoted, hence the check.
pub async fn curl(token: Option<&str>, headers: &[&str], args: &[&str]) -> io::Result<Output> {
    let mut config = String::new();
    if let Some(token) = token {
        if token.is_empty()
            || !token
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-._~+/=".contains(&b))
        {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        config.push_str("header = \"Authorization: Bearer ");
        config.push_str(token);
        config.push_str("\"\n");
    }
    for header in headers {
        config.push_str("header = \"");
        config.push_str(&header.replace('\\', "\\\\").replace('"', "\\\""));
        config.push_str("\"\n");
    }
    let mut child = Command::new("curl")
        .args([
            "--disable",
            "--silent",
            "--max-time",
            "12",
            "--max-filesize",
            RESPONSE_LIMIT,
            "--config",
            "-",
        ])
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("stdin unavailable"))?;
    stdin.write_all(config.as_bytes()).await?;
    stdin.shutdown().await?;
    drop(stdin);
    tokio::time::timeout(Duration::from_secs(20), child.wait_with_output()).await?
}

#[cfg(feature = "native-http")]
pub async fn get(url: &str) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(25))
        .user_agent("dogcodes/agent-updates")
        .build()
        .map_err(|_| "backendText.versionCheckInit")?;
    let mut response = client
        .get(url)
        .send()
        .await
        .map_err(|_| "backendText.versionSourceConnect")?
        .error_for_status()
        .map_err(|_| "backendText.versionSourceDown")?;
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "backendText.versionReadFailed")?
    {
        if body.len() + chunk.len() > 1024 * 1024 {
            return Err("backendText.versionTooLarge".into());
        }
        body.extend_from_slice(&chunk);
    }
    String::from_utf8(body).map_err(|_| "backendText.versionBadEncoding".into())
}

#[cfg(not(feature = "native-http"))]
pub async fn get(url: &str) -> Result<String, String> {
    let output = curl(
        None,
        &["User-Agent: dogcodes/agent-updates"],
        &["--fail", "--location", url],
    )
    .await
    .map_err(|_| "backendText.versionSourceConnect")?;
    if !output.status.success() {
        return Err("backendText.versionSourceDown".into());
    }
    String::from_utf8(output.stdout).map_err(|_| "backendText.versionBadEncoding".into())
}
