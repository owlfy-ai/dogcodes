#[tauri::command]
pub async fn open_website(url: String) -> Result<(), String> {
    let parsed = tauri::Url::parse(&url).map_err(|_| "无效的网站地址".to_string())?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err("仅支持 HTTP 和 HTTPS 网站链接".into());
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
        .map_err(|_| "无法启动浏览器".to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err("无法打开网站，请检查默认浏览器设置".into())
    }
}
