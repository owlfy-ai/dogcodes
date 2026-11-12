use base64::{Engine, engine::general_purpose::STANDARD};
use tauri_plugin_dialog::DialogExt;

#[tauri::command]
pub async fn core_export_attachment(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    name: String,
    bytes: Vec<u8>,
) -> Result<bool, String> {
    if bytes.len() > 25 * 1024 * 1024 {
        return Err("backendText.m962d0a69".into());
    }
    let filename = std::path::Path::new(&name)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .ok_or("backendText.mf04fa37b")?;
    let (sender, receiver) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_parent(&window)
        .set_title("backendText.m8894375c")
        .set_file_name(filename)
        .save_file(move |path| {
            let _ = sender.send(path);
        });
    let Some(path) = receiver.await.map_err(|_| "backendText.m101ee925")? else {
        return Ok(false);
    };
    let path = path.into_path().map_err(|_| "backendText.mf66f180b")?;
    tokio::fs::write(path, bytes)
        .await
        .map_err(|_| "backendText.mdb0d7014")?;
    Ok(true)
}

#[tauri::command]
pub async fn core_save_attachment(
    state: tauri::State<'_, crate::DesktopState>,
    request: agent_protocol::AttachmentUploadRequest,
    payload_base64: String,
) -> Result<agent_domain::PromptAttachment, String> {
    state.core.attachment_save(request, payload_base64).await
}

#[tauri::command]
pub async fn core_attachment_upload_receipt(
    state: tauri::State<'_, crate::DesktopState>,
    request: agent_protocol::AttachmentUploadRequest,
) -> Result<Option<agent_domain::PromptAttachment>, String> {
    state.core.attachment_receipt(request).await
}

/// Byte reads and their managed integrity checks stay in the background service.
#[tauri::command]
pub async fn core_read_attachment(
    state: tauri::State<'_, crate::DesktopState>,
    path: String,
) -> Result<Vec<u8>, String> {
    let encoded = state.core.attachment_read(path).await?;
    STANDARD
        .decode(encoded)
        .map_err(|_| "backendText.mb7e7d559".into())
}
