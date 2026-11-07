//! Reads the clipboard for the app's own Paste menu item. A web view can only ask with a
//! system prompt of its own, and it can't read images that way at all.
use base64::{Engine, engine::general_purpose::STANDARD};
use dogcodes_remote_desktop::{clipboard_image_for_paste, clipboard_text_for_paste};
use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardContent {
    text: Option<String>,
    /// A PNG in base64, only when asked for and there is no text to paste instead.
    image: Option<String>,
}

#[tauri::command]
pub async fn read_clipboard(include_image: bool) -> Result<ClipboardContent, String> {
    tokio::task::spawn_blocking(move || {
        let text = clipboard_text_for_paste()?.filter(|text| !text.is_empty());
        let image = if include_image && text.is_none() {
            clipboard_image_for_paste()?.map(|bytes| STANDARD.encode(bytes))
        } else {
            None
        };
        Ok(ClipboardContent { text, image })
    })
    .await
    .map_err(|_| "无法读取电脑剪贴板，请重新复制后重试".to_owned())?
}
