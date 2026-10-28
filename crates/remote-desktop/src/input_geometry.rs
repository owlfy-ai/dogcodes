//! Fresh input bounds, independent of capture objects and encoded resolution.
use crate::{Display, Input, Result};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Bounds {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}
impl Bounds {
    fn apply(self, original: &Display) -> Result<Display> {
        if self.width == 0
            || self.height == 0
            || i64::from(self.x) + i64::from(self.width) - 1 > i64::from(i32::MAX)
            || i64::from(self.y) + i64::from(self.height) - 1 > i64::from(i32::MAX)
        {
            return Err("显示器当前坐标范围无效".into());
        }
        Ok(Display {
            x: self.x,
            y: self.y,
            width: self.width,
            height: self.height,
            ..original.clone()
        })
    }
}
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;
#[cfg(target_os = "linux")]
use linux as platform;
#[cfg(target_os = "macos")]
use macos as platform;
pub(super) use platform::{Context, Resolver};
#[cfg(windows)]
use windows as platform;

pub(super) fn needs_bounds(events: &[Input]) -> bool {
    events.iter().any(|e| match e {
        Input::Move { .. } | Input::Scroll { .. } => true,
        Input::Button { direction, .. } => direction != "release",
        _ => false,
    })
}
#[derive(Default)]
pub(super) struct Pointer(Option<(u32, u16, u16, Bounds)>);
impl Pointer {
    pub fn remember(&mut self, display: &Display, x: u16, y: u16) {
        self.0 = Some((
            display.id,
            x,
            y,
            Bounds {
                x: display.x,
                y: display.y,
                width: display.width,
                height: display.height,
            },
        ));
    }
    pub fn changed(&self, display: &Display) -> Option<(u16, u16)> {
        self.0
            .filter(|(id, _, _, bounds)| {
                *id == display.id
                    && *bounds
                        != Bounds {
                            x: display.x,
                            y: display.y,
                            width: display.width,
                            height: display.height,
                        }
            })
            .map(|(_, x, y, _)| (x, y))
    }
}

/// Take one current snapshot for the whole ordered batch, before injecting
/// anything. Text/keys and releases must remain usable after display removal.
#[cfg(test)]
pub(super) fn batch(
    original: &Display,
    events: Vec<Input>,
    current: impl FnOnce(&Display) -> Result<Display>,
    send: impl FnMut(&Display, Input) -> Result<()>,
) -> Result<()> {
    batch_guarded(original, events, current, || true, send)
}
pub(super) fn batch_guarded(
    original: &Display,
    events: Vec<Input>,
    current: impl FnOnce(&Display) -> Result<Display>,
    active: impl Fn() -> bool,
    mut send: impl FnMut(&Display, Input) -> Result<()>,
) -> Result<()> {
    if !active() {
        return Err("输入请求已结束".into());
    }
    let display = if needs_bounds(&events) {
        current(original)?
    } else {
        original.clone()
    };
    if display.id != original.id {
        return Err("显示器身份已变化，禁止使用另一屏幕的坐标".into());
    }
    if !active() {
        return Err("输入请求已结束".into());
    }
    events.into_iter().try_for_each(|e| {
        if !active() {
            return Err("输入请求已结束".into());
        }
        send(&display, e)
    })
}

#[cfg(test)]
mod tests;
