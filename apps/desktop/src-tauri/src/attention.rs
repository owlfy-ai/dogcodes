//! Tells someone in another app that a conversation needs them, and takes them there when
//! they click. A web view has no working notification API in a desktop window.
use std::sync::{
    Mutex, OnceLock,
    atomic::{AtomicUsize, Ordering},
};
use tauri::{AppHandle, Emitter, EventTarget, Manager};

const MAIN_WINDOW: &str = "main";
/// A notification nobody has answered holds a thread until it's cleared, which on macOS can
/// be days in Notification Center. Past this one still appears, without opening anything.
const MAX_AWAITING_CLICK: usize = 32;
static AWAITING_CLICK: AtomicUsize = AtomicUsize::new(0);

fn claim_slot(count: &AtomicUsize, limit: usize) -> bool {
    count
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
            (current < limit).then_some(current + 1)
        })
        .is_ok()
}

/// Clicks that arrive before the page can hear them, such as the one that launched the app.
#[derive(Default)]
struct ClickInbox {
    page_ready: bool,
    waiting: Option<String>,
}

impl ClickInbox {
    /// The click to tell the page about now; otherwise it's kept until the page asks.
    fn receive(&mut self, task_id: String) -> Option<String> {
        if self.page_ready {
            return Some(task_id);
        }
        self.waiting = Some(task_id);
        None
    }

    fn page_ready(&mut self) -> Option<String> {
        self.page_ready = true;
        self.waiting.take()
    }
}

static INBOX: Mutex<ClickInbox> = Mutex::new(ClickInbox {
    page_ready: false,
    waiting: None,
});
static APP: OnceLock<AppHandle> = OnceLock::new();

fn open_conversation(task_id: String) {
    let Some(app) = APP.get() else { return };
    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
    let ready = INBOX
        .lock()
        .map_or(None, |mut inbox| inbox.receive(task_id));
    if let Some(task_id) = ready {
        let _ = app.emit_to(
            EventTarget::webview_window(MAIN_WINDOW),
            "attention-open",
            task_id,
        );
    }
}

/// Call during startup.
pub fn install(app: &AppHandle) {
    let _ = APP.set(app.clone());
    #[cfg(target_os = "macos")]
    if notification_center::available() {
        notification_center::install(open_conversation);
    }
}

#[tauri::command]
pub fn take_attention_click() -> Option<String> {
    INBOX.lock().ok()?.page_ready()
}

/// Whether the notification was clicked; always false unless `await_click` is set.
/// Only for builds outside the notification center. Replaced notifications and withdrawal
/// aren't possible here, so they pile up until someone clears them.
#[cfg(target_os = "macos")]
fn show(app: &AppHandle, title: &str, body: &str, await_click: bool) -> bool {
    use mac_notification_sys::{Notification, NotificationResponse, set_application};
    // Set once, and only the first call counts. An unbundled dev build has no identity of
    // its own for the system to notify under.
    let _ = set_application(if tauri::is_dev() {
        "com.apple.Terminal"
    } else {
        &app.config().identifier
    });
    matches!(
        Notification::new()
            .title(title)
            .message(body)
            .wait_for_click(await_click)
            .send(),
        Ok(NotificationResponse::Click)
    )
}

/// Whether the notification was clicked; always false unless `await_click` is set.
#[cfg(not(target_os = "macos"))]
fn show(app: &AppHandle, title: &str, body: &str, await_click: bool) -> bool {
    use notify_rust::{Notification, NotificationResponse};
    let mut notification = Notification::new();
    notification.summary(title).body(body);
    // Windows only knows an identifier the installer registered.
    #[cfg(windows)]
    if !tauri::is_dev() {
        notification.app_id(&app.config().identifier);
    }
    // Clicking the body is reported only when the server is offered a default action.
    #[cfg(unix)]
    {
        let _ = app;
        notification.appname("dog.codes").action("default", "");
    }
    let Ok(handle) = notification.show() else {
        return false;
    };
    if !await_click {
        return false;
    }
    let clicked = std::cell::Cell::new(false);
    let _ = handle.wait_for_response(|response: &NotificationResponse| {
        clicked.set(matches!(
            response,
            NotificationResponse::Default | NotificationResponse::Action(_)
        ));
    });
    clicked.get()
}

fn notify_on_thread(app: AppHandle, task_id: String, title: String, body: String) {
    let await_click = claim_slot(&AWAITING_CLICK, MAX_AWAITING_CLICK);
    let notification = std::thread::Builder::new()
        .name("attention-notification".into())
        .spawn(move || {
            if show(&app, &title, &body, await_click) {
                open_conversation(task_id);
            }
            if await_click {
                AWAITING_CLICK.fetch_sub(1, Ordering::AcqRel);
            }
        });
    if notification.is_err() && await_click {
        AWAITING_CLICK.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Best effort: a refused notification mustn't get in the way of the conversation, and the
/// sidebar still shows the same state.
#[tauri::command]
pub fn notify_attention(app: AppHandle, task_id: String, title: String, body: String) {
    #[cfg(target_os = "macos")]
    if notification_center::available() {
        let (id, heading, text) = (task_id.clone(), title.clone(), body.clone());
        notification_center::send(&id, &heading, &text, move || {
            notify_on_thread(app, task_id, title, body);
        });
        return;
    }
    notify_on_thread(app, task_id, title, body);
}

#[tauri::command]
pub fn withdraw_attention(task_id: String) {
    #[cfg(target_os = "macos")]
    if notification_center::available() {
        notification_center::withdraw(&task_id);
    }
    #[cfg(not(target_os = "macos"))]
    let _ = task_id;
}

/// Windows has no count badge; there the sidebar is the only indication.
#[tauri::command]
pub fn set_attention_badge(window: tauri::WebviewWindow, count: u32) {
    let _ = window.set_badge_count((count > 0).then_some(i64::from(count)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn holds_a_click_until_the_page_can_hear_it() {
        let mut inbox = ClickInbox::default();
        assert_eq!(inbox.receive("first".into()), None);
        assert_eq!(inbox.receive("second".into()), None);
        assert_eq!(inbox.page_ready().as_deref(), Some("second"));
        assert_eq!(inbox.page_ready(), None);
        assert_eq!(inbox.receive("third".into()).as_deref(), Some("third"));
    }

    #[test]
    fn stops_waiting_on_clicks_at_the_limit() {
        let count = AtomicUsize::new(0);
        assert!(claim_slot(&count, 2));
        assert!(claim_slot(&count, 2));
        assert!(!claim_slot(&count, 2));
        assert_eq!(count.load(Ordering::Acquire), 2);
        count.fetch_sub(1, Ordering::AcqRel);
        assert!(claim_slot(&count, 2));
    }
}
