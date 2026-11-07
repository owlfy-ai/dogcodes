//! macOS `UNUserNotificationCenter`. Other systems have nothing here.
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::{available, install, send, withdraw};
