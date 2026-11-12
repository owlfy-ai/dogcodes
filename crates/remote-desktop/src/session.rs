#[cfg(any(target_os = "macos", windows))]
use super::Result;
#[cfg(target_os = "macos")]
fn output(program: &str, args: &[&str]) -> Result<Vec<u8>> {
    use std::{
        io::Read,
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    let mut child = Command::new(program)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "backendText.m07442bb3")?;
    let stdout = child.stdout.take().ok_or("backendText.me3cd80f8")?;
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .take(256 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let start = Instant::now();
    loop {
        match child.try_wait().map_err(|_| "backendText.m07442bb3")? {
            Some(status) => {
                if !status.success() {
                    return Err("backendText.m07442bb3".into());
                }
                let bytes = reader
                    .join()
                    .map_err(|_| "backendText.me3cd80f8")?
                    .map_err(|_| "backendText.me3cd80f8")?;
                if bytes.len() > 256 * 1024 {
                    return Err("backendText.mdc4210c1".into());
                }
                return Ok(bytes);
            }
            None if start.elapsed() > Duration::from_secs(2) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("backendText.m3a1f59bd".into());
            }
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    }
}
/// The console session's lock flag straight from WindowServer: the same
/// `CGSSessionScreenIsLocked` property ioreg reports, without spawning a
/// process (~28 ms each), so it can guard every frame and input batch.
#[cfg(target_os = "macos")]
fn session_locked() -> Option<bool> {
    use core_foundation::{
        base::{CFType, TCFType},
        boolean::CFBoolean,
        dictionary::{CFDictionary, CFDictionaryRef},
        string::CFString,
    };
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGSessionCopyCurrentDictionary() -> CFDictionaryRef;
    }
    // SAFETY: the function takes no arguments and returns a +1 retained
    // dictionary or NULL (no window-server session); create_rule takes that
    // ownership exactly once.
    let session: CFDictionary<CFString, CFType> = unsafe {
        let raw = CGSessionCopyCurrentDictionary();
        if raw.is_null() {
            return None;
        }
        CFDictionary::wrap_under_create_rule(raw)
    };
    let flag = |key: &str| {
        session
            .find(CFString::new(key))
            .map(|value| value.downcast::<CFBoolean>().map(bool::from))
    };
    if flag("kCGSSessionOnConsoleKey")? != Some(true) {
        return None;
    }
    match flag("CGSSessionScreenIsLocked") {
        // macOS omits this property for the unlocked console session.
        None => Some(false),
        Some(value) => value,
    }
}
#[cfg(target_os = "macos")]
pub fn locked() -> Result<bool> {
    if let Some(locked) = session_locked() {
        return Ok(locked);
    }
    let bytes = output("/usr/sbin/ioreg", &["-n", "Root", "-d1", "-a"])?;
    let value =
        plist::Value::from_reader(std::io::Cursor::new(bytes)).map_err(|_| "backendText.m3a0c2ef1")?;
    let root = value.as_dictionary().or_else(|| {
        value
            .as_array()
            .and_then(|a| a.first())
            .and_then(plist::Value::as_dictionary)
    });
    let sessions = root
        .and_then(|d| d.get("IOConsoleUsers"))
        .and_then(plist::Value::as_array)
        .ok_or("backendText.mcc7385e4")?;
    for session in sessions {
        if let Some(session) = session.as_dictionary()
            && session
                .get("kCGSSessionOnConsoleKey")
                .and_then(plist::Value::as_boolean)
                == Some(true)
        {
            // macOS omits this property for the unlocked console session.
            return match session.get("CGSSessionScreenIsLocked") {
                None => Ok(false),
                Some(value) => value
                    .as_boolean()
                    .ok_or_else(|| "backendText.ma77f32d6".into()),
            };
        }
    }
    Err("backendText.me3b44c94".into())
}
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::locked;
#[cfg(all(target_os = "linux", test))]
pub(crate) async fn fixture_login1_query(
    connection: &zbus::Connection,
    session: Option<&str>,
    pid: u32,
    uid: u32,
) -> crate::Result<bool> {
    linux::query(connection, session, pid, uid).await
}

#[cfg(any(target_os = "linux", test))]
mod query;
#[cfg(windows)]
pub fn locked() -> Result<bool> {
    use windows::Win32::System::RemoteDesktop::{
        WTS_CURRENT_SESSION, WTSFreeMemory, WTSINFOEXW, WTSQuerySessionInformationW,
        WTSSessionInfoEx,
    };
    let mut buffer = windows::core::PWSTR::null();
    let mut bytes = 0;
    // SAFETY: WTS allocates the returned buffer; we validate size/level before
    // reading and release it once with the matching WTSFreeMemory function.
    unsafe {
        WTSQuerySessionInformationW(
            None,
            WTS_CURRENT_SESSION,
            WTSSessionInfoEx,
            &mut buffer,
            &mut bytes,
        )
        .map_err(|_| "backendText.m07442bb3")?;
        let result = if !buffer.is_null() && bytes as usize >= std::mem::size_of::<WTSINFOEXW>() {
            let info = &*buffer.0.cast::<WTSINFOEXW>();
            if info.Level != 1 {
                Err("backendText.ma77f32d6".into())
            } else {
                match info.Data.WTSInfoExLevel1.SessionFlags {
                    0 => Ok(true),
                    1 => Ok(false),
                    _ => Err("backendText.ma77f32d6".into()),
                }
            }
        } else {
            Err("backendText.ma77f32d6".into())
        };
        if !buffer.is_null() {
            WTSFreeMemory(buffer.0.cast());
        }
        result
    }
}
