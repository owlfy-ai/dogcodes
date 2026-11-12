#[tauri::command]
pub async fn open_website(url: String) -> Result<(), String> {
    let parsed = tauri::Url::parse(&url).map_err(|_| "backendText.m375a9f81".to_string())?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err("backendText.m63b0fcd8".into());
    }
    #[cfg(target_os = "macos")]
    let mut command = tokio::process::Command::new("/usr/bin/open");
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = tokio::process::Command::new("rundll32.exe");
        command.arg("url.dll,FileProtocolHandler");
        command
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut command = tokio::process::Command::new("xdg-open");
    let status = command
        .arg(parsed.as_str())
        .status()
        .await
        .map_err(|_| "backendText.mbd4e2081".to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err("backendText.m0b0f81bc".into())
    }
}
