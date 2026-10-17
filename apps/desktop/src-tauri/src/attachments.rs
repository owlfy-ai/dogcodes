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
        return Err("附件超过 25 MB".into());
    }
    let filename = std::path::Path::new(&name)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .ok_or("附件文件名无效")?;
    let (sender, receiver) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_parent(&window)
        .set_title("下载图片")
        .set_file_name(filename)
        .save_file(move |path| {
            let _ = sender.send(path);
        });
    let Some(path) = receiver.await.map_err(|_| "保存窗口已关闭")? else {
        return Ok(false);
    };
    let path = path.into_path().map_err(|_| "请选择本地文件")?;
    tokio::fs::write(path, bytes)
        .await
        .map_err(|_| "图片保存失败")?;
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
        .map_err(|_| "附件读取结果无效".into())
}
