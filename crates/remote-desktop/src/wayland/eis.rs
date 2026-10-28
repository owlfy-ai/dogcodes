//! Session-owned EI sender. Never opens an environment socket or a second grant.
//! Regions/keymaps are immutable per device; replacement invalidates queued input.
mod keyboard;
#[cfg(test)]
pub(super) mod tests;

use super::{State, StreamInfo, input};
use crate::{Input, Result};
use futures::{FutureExt, StreamExt};
use reis::{
    ei,
    event::{Connection, Device, DeviceCapability as Cap, EiEvent},
};
use std::{collections::HashMap, os::fd::OwnedFd, sync::atomic::Ordering};

pub(super) struct Sender {
    connection: Connection,
    events: reis::tokio::EiEventStream,
    converter: reis::event::EiEventConverter,
    devices: Vec<DeviceState>,
    sequence: u32,
    held: Held,
    focus: super::focus::Probe,
}
struct DeviceState {
    device: Device,
    resumed: bool,
    keyboard: Option<keyboard::Map>,
}
#[derive(Clone, Default)]
struct Held {
    keys: HashMap<i32, Key>,
    buttons: HashMap<i32, Device>,
}
#[derive(Clone)]
struct Key {
    device: Device,
    code: u32,
    physical: bool,
}
enum Kind {
    Move(f32, f32),
    Button(u32, bool),
    Key(u32, bool, bool), // code, physical, down
    Scroll(i32, i32),
    Text(String),
    Synchronize,
    Paste {
        text: String,
        ctrl: u32,
        v: u32,
        shift: Option<u32>,
    },
}
struct Action {
    device: Device,
    kind: Kind,
}
pub(super) struct Prepared {
    generation: u64,
    actions: Vec<Action>,
    held: Held,
}

impl Sender {
    pub(super) async fn initialize(&mut self, state: &State, streams: &[StreamInfo]) -> Result<()> {
        loop {
            if streams.iter().all(|stream| {
                self.pointer(stream).is_ok_and(|(d, _)| {
                    d.resumed
                        && self.capability(d, Cap::Button).is_ok()
                        && self.capability(d, Cap::Scroll).is_ok()
                        && (self.capability(d, Cap::Text).is_ok()
                            || self
                                .capability(d, Cap::Keyboard)
                                .is_ok_and(|k| k.keyboard.is_some()))
                })
            }) {
                return Ok(());
            }
            let event = self.next().await;
            self.handle(state, event)?;
        }
    }
    pub(super) async fn open(fd: OwnedFd) -> Result<Self> {
        let context = ei::Context::new(std::os::unix::net::UnixStream::from(fd))
            .map_err(|_| "无法连接已授权的 Wayland EIS 输入服务")?;
        let mut events = reis::tokio::EiEventStream::new(context.clone())
            .map_err(|_| "无法监听 EIS 输入服务")?;
        let handshake = reis::tokio::ei_handshake(
            &mut events,
            "DogCodes remote desktop",
            ei::handshake::ContextType::Sender,
        )
        .await
        .map_err(|_| "Wayland EIS 握手失败，请重新连接并授权")?;
        let converter = reis::event::EiEventConverter::new(&context, handshake);
        let connection = converter.connection().clone();
        Ok(Self {
            connection,
            events,
            converter,
            devices: Vec::new(),
            sequence: 0,
            held: Held::default(),
            focus: super::focus::Probe::default(),
        })
    }
    pub(super) async fn next(&mut self) -> Option<std::result::Result<EiEvent, reis::Error>> {
        loop {
            if let Some(event) = self.converter.next_event() {
                return Some(Ok(event));
            }
            match self.events.next().await? {
                Ok(reis::PendingRequestResult::Request(event)) => {
                    if let Err(error) = self.converter.handle_event(event) {
                        return Some(Err(error.into()));
                    }
                }
                Ok(reis::PendingRequestResult::ParseError(error)) => {
                    return Some(Err(error.into()));
                }
                Ok(reis::PendingRequestResult::InvalidObject(_)) => {
                    return Some(Err(std::io::Error::other("invalid EI object").into()));
                }
                Err(error) => return Some(Err(error.into())),
            }
        }
    }
    pub(super) fn handle(
        &mut self,
        state: &State,
        event: Option<std::result::Result<EiEvent, reis::Error>>,
    ) -> Result<()> {
        let event = event
            .ok_or("Wayland EIS 连接已关闭，请重新连接并授权")?
            .map_err(|_| "Wayland EIS 协议中断，请重新连接并授权")?;
        match event {
            EiEvent::Disconnected(_) => return Err("电脑已断开 EIS 输入授权，请重新连接".into()),
            EiEvent::SeatAdded(e) => {
                e.seat.bind_capabilities(
                    Cap::PointerAbsolute | Cap::Button | Cap::Scroll | Cap::Keyboard | Cap::Text,
                );
            }
            EiEvent::SeatRemoved(e) => {
                self.devices.retain(|d| d.device.seat() != &e.seat);
                self.drop_stale_held();
            }
            EiEvent::DeviceAdded(e) => {
                if self.devices.len() >= 64 {
                    return Err("Wayland 输入设备数量超出限制".into());
                }
                let keyboard = e.device.keymap().map(keyboard::Map::read).transpose()?;
                if e.device.device().version() >= 3 {
                    e.device.device().ready();
                }
                self.devices.push(DeviceState {
                    device: e.device,
                    resumed: false,
                    keyboard,
                });
            }
            EiEvent::DeviceRemoved(e) => {
                self.devices.retain(|d| d.device != e.device);
                self.drop_stale_held();
            }
            EiEvent::DevicePaused(e) => {
                if let Some(d) = self.devices.iter_mut().find(|d| d.device == e.device) {
                    d.resumed = false;
                    if let Some(map) = d.keyboard.as_mut() {
                        map.reset();
                    }
                }
                // The compositor resets all held keys/buttons on pause. No input
                // (including a release) may be sent until this device resumes.
                self.drop_stale_held();
            }
            EiEvent::DeviceResumed(e) => {
                if let Some(d) = self.devices.iter_mut().find(|d| d.device == e.device) {
                    if d.resumed {
                        return Err("Wayland 输入设备重复恢复，已结束远控".into());
                    }
                    self.sequence = self.sequence.wrapping_add(1);
                    d.device
                        .device()
                        .start_emulating(self.connection.serial(), self.sequence);
                    d.resumed = true;
                }
            }
            EiEvent::KeyboardModifiers(e) => {
                if let Some(d) = self.devices.iter_mut().find(|d| d.device == e.device)
                    && let Some(map) = d.keyboard.as_mut()
                    && map.modifiers(e.depressed, e.latched, e.locked, e.group)
                {
                    return Ok(());
                }
            }
            _ => return Ok(()),
        }
        state.input_generation.fetch_add(1, Ordering::AcqRel);
        self.flush()
    }
    fn drop_stale_held(&mut self) {
        let active = |device: &Device| {
            self.devices
                .iter()
                .any(|d| d.resumed && &d.device == device)
        };
        self.held.keys.retain(|_, key| active(&key.device));
        self.held.buttons.retain(|_, device| active(device));
    }
    pub(super) fn drain(&mut self, state: &State) -> Result<()> {
        // Bounded so a malicious/broken EIS cannot starve cancellation or D-Bus.
        for _ in 0..256 {
            match self.next().now_or_never() {
                Some(event) => self.handle(state, event)?,
                None => return Ok(()),
            }
        }
        Err("Wayland 输入设备变化过于频繁，请重新连接".into())
    }
    fn flush(&self) -> Result<()> {
        self.connection
            .flush()
            .map_err(|_| "Wayland EIS 发送失败，结果未知，禁止重放".into())
    }
    fn pointer(&self, stream: &StreamInfo) -> Result<(&DeviceState, &reis::event::Region)> {
        let mapping = stream
            .mapping_id
            .as_deref()
            .ok_or("屏幕缺少可信输入区域，请重新授权")?;
        let mut found = None;
        for d in &self.devices {
            if d.device.device_type() != ei::device::DeviceType::Virtual
                || d.device.interface::<ei::PointerAbsolute>().is_none()
            {
                continue;
            }
            for region in d
                .device
                .regions()
                .iter()
                .filter(|r| r.mapping_id.as_deref() == Some(mapping))
            {
                if found.is_some() {
                    return Err("屏幕对应多个输入区域，无法安全控制".into());
                }
                validate_region(region)?;
                found = Some((d, region));
            }
        }
        found.ok_or_else(|| "屏幕输入区域正在更新，请稍后重试".into())
    }
    fn capability(&self, pointer: &DeviceState, cap: Cap) -> Result<&DeviceState> {
        if pointer.resumed && pointer.device.has_capability(cap) {
            // Return a reference owned by `self`, not the caller's parameter.
            return self
                .devices
                .iter()
                .find(|d| d.device == pointer.device)
                .ok_or_else(|| "Wayland 输入设备已替换".into());
        }
        let mut candidates = self.devices.iter().filter(|d| {
            d.resumed && d.device.seat() == pointer.device.seat() && d.device.has_capability(cap)
        });
        let device = candidates
            .next()
            .ok_or("Wayland 输入设备尚未就绪，请稍后重试")?;
        if candidates.next().is_some() {
            return Err("Wayland 同一座席存在多个输入设备，无法安全选择".into());
        }
        Ok(device)
    }
    #[cfg(test)]
    pub(super) fn prepare(
        &self,
        state: &State,
        stream: &StreamInfo,
        generation: u64,
        raw: Vec<Input>,
    ) -> Result<Prepared> {
        self.prepare_with_clipboard(state, stream, generation, raw, false)
    }
    pub(super) fn prepare_with_clipboard(
        &self,
        state: &State,
        stream: &StreamInfo,
        generation: u64,
        raw: Vec<Input>,
        clipboard: bool,
    ) -> Result<Prepared> {
        if state.input_generation.load(Ordering::Acquire) != generation {
            return Err("屏幕或键盘配置已变化，本次输入未执行，请重试".into());
        }
        let mut actions = Vec::new();
        let mut held = self.held.clone();
        let mut maps = Vec::new();
        let mut shortcut_before_text = false;
        // Releases use their original device even after the user switches monitors.
        // Any other input must resolve the selected stream's mapping, never a node,
        // monitor name, matching dimensions, or the first device in a list.
        for event in raw {
            match event {
                Input::Move { x, y } => {
                    let (d, r) = self.pointer(stream)?;
                    if !d.resumed {
                        return Err("屏幕输入已暂停，请稍后重试".into());
                    }
                    actions.push(Action {
                        device: d.device.clone(),
                        kind: Kind::Move(
                            r.x as f32 + f32::from(x) * (r.width - 1) as f32 / 65535.,
                            r.y as f32 + f32::from(y) * (r.height - 1) as f32 / 65535.,
                        ),
                    });
                }
                Input::Button { button, direction } => {
                    let code = match button.as_str() {
                        "left" => 0x110,
                        "right" => 0x111,
                        "middle" => 0x112,
                        _ => return Err("鼠标按钮无效".into()),
                    };
                    if direction == "click" && held.buttons.contains_key(&code) {
                        return Err("请先松开已按下的鼠标按钮，再点击".into());
                    }
                    for down in directions(&direction)? {
                        if down {
                            if held.buttons.contains_key(&code) {
                                continue;
                            }
                            let (pointer, _) = self.pointer(stream)?;
                            let d = self.capability(pointer, Cap::Button)?;
                            actions.push(Action {
                                device: d.device.clone(),
                                kind: Kind::Button(code as u32, true),
                            });
                            held.buttons.insert(code, d.device.clone());
                        } else if let Some(device) = held.buttons.remove(&code) {
                            actions.push(Action {
                                device,
                                kind: Kind::Button(code as u32, false),
                            });
                        }
                    }
                }
                Input::Key { key, direction } => {
                    shortcut_before_text = true;
                    let sym = input::keysym(&key)?;
                    if direction == "click" && held.keys.contains_key(&sym) {
                        return Err("请先松开已按下的按键，再点击".into());
                    }
                    for down in directions(&direction)? {
                        if down {
                            if held.keys.contains_key(&sym) {
                                continue;
                            }
                            let (pointer, _) = self.pointer(stream)?;
                            let mapped = self.key(pointer, sym, &mut maps)?;
                            if mapped.physical
                                && let Some(map) = self.planning_map(&mapped.device, &mut maps)
                            {
                                map.note_key(mapped.code, true);
                            }
                            actions.push(Action {
                                device: mapped.device.clone(),
                                kind: Kind::Key(mapped.code, mapped.physical, true),
                            });
                            held.keys.insert(sym, mapped);
                        } else if let Some(key) = held.keys.remove(&sym) {
                            if key.physical
                                && let Some(map) = self.planning_map(&key.device, &mut maps)
                            {
                                map.note_key(key.code, false);
                            }
                            actions.push(Action {
                                device: key.device,
                                kind: Kind::Key(key.code, key.physical, false),
                            });
                        }
                    }
                }
                Input::Text { text } => {
                    if text.len() > 64 * 1024 || text.contains('\0') {
                        return Err("键入文字过长或包含不支持的空字符".into());
                    }
                    if text.is_empty() {
                        continue;
                    }
                    let (pointer, _) = self.pointer(stream)?;
                    if shortcut_before_text {
                        actions.push(Action {
                            device: pointer.device.clone(),
                            kind: Kind::Synchronize,
                        });
                        shortcut_before_text = false;
                    }
                    if let Ok(d) = self.capability(pointer, Cap::Text) {
                        for part in text_chunks(&text) {
                            actions.push(Action {
                                device: d.device.clone(),
                                kind: Kind::Text(part.to_owned()),
                            });
                        }
                    } else {
                        let d = self.capability(pointer, Cap::Keyboard)?;
                        let map = self
                            .planning_map(&d.device, &mut maps)
                            .ok_or("系统没有提供键盘布局，无法输入文字")?;
                        if !held.keys.is_empty() {
                            return Err("请先松开快捷键，再输入文字".into());
                        }
                        let planned = match map.plan_text(&text) {
                            Ok(events) => events,
                            Err(keyboard::TextError::Unmappable) if clipboard => {
                                actions.push(Action {
                                    device: d.device.clone(),
                                    kind: Kind::Paste {
                                        text,
                                        ctrl: map.key(0xffe3)?,
                                        v: map.key('v' as i32)?,
                                        shift: map.key(0xffe1).ok(),
                                    },
                                });
                                Vec::new()
                            }
                            Err(error) => return Err(error.message()),
                        };
                        for (code, down) in planned {
                            map.note_key(code, down);
                            actions.push(Action {
                                device: d.device.clone(),
                                kind: Kind::Key(code, true, down),
                            });
                        }
                    }
                }
                Input::Scroll { x, y } => {
                    if x == 0 && y == 0 {
                        continue;
                    }
                    let (pointer, _) = self.pointer(stream)?;
                    let d = self.capability(pointer, Cap::Scroll)?;
                    actions.push(Action {
                        device: d.device.clone(),
                        kind: Kind::Scroll(i32::from(x) * 120, i32::from(y) * 120),
                    });
                }
            }
            if actions.len() > 32768 {
                return Err("单次键鼠操作过多".into());
            }
        }
        Ok(Prepared {
            generation,
            actions,
            held,
        })
    }
    fn planning_map<'a>(
        &self,
        device: &Device,
        maps: &'a mut Vec<(Device, keyboard::Map)>,
    ) -> Option<&'a mut keyboard::Map> {
        let original = self
            .devices
            .iter()
            .find(|d| &d.device == device)?
            .keyboard
            .as_ref()?;
        let at = match maps.iter().position(|(d, _)| d == device) {
            Some(at) => at,
            None => {
                maps.push((device.clone(), original.fork()));
                maps.len() - 1
            }
        };
        Some(&mut maps[at].1)
    }
    fn key(
        &self,
        pointer: &DeviceState,
        sym: i32,
        maps: &mut Vec<(Device, keyboard::Map)>,
    ) -> Result<Key> {
        // Real shortcut keys use the compositor keymap whenever possible; text
        // stays on ei_text. This also lets modifier feedback be checked against
        // the precise physical transitions generated by this sender.
        if let Ok(d) = self.capability(pointer, Cap::Keyboard)
            && let Some(map) = self.planning_map(&d.device, maps)
            && let Ok(code) = map.key(sym)
        {
            return Ok(Key {
                device: d.device.clone(),
                code,
                physical: true,
            });
        }
        if let Ok(d) = self.capability(pointer, Cap::Text) {
            return Ok(Key {
                device: d.device.clone(),
                code: sym as u32,
                physical: false,
            });
        }
        let d = self.capability(pointer, Cap::Keyboard)?;
        let code = self
            .planning_map(&d.device, maps)
            .ok_or("系统没有提供键盘布局，无法映射按键")?
            .key(sym)?;
        Ok(Key {
            device: d.device.clone(),
            code,
            physical: true,
        })
    }
    pub(super) async fn apply(&mut self, state: &State, prepared: Prepared) -> Result<()> {
        self.apply_with_clipboard(state, prepared, None).await
    }
    pub(super) async fn apply_with_clipboard(
        &mut self,
        state: &State,
        prepared: Prepared,
        clipboard: Option<&super::clipboard::Clipboard>,
    ) -> Result<()> {
        for action in prepared.actions {
            tokio::task::yield_now().await;
            self.drain(state)?;
            if !state.active()
                || state.input_generation.load(Ordering::Acquire) != prepared.generation
            {
                return Err("输入期间授权或设备配置已变化，已结束本次远控，禁止重放".into());
            }
            let d = &action.device;
            if !self.devices.iter().any(|v| v.resumed && &v.device == d) || !d.device().is_alive() {
                return Err("Wayland 输入设备已暂停或移除".into());
            }
            match action.kind {
                Kind::Paste {
                    text,
                    ctrl,
                    v,
                    shift,
                } => {
                    let clipboard = clipboard.ok_or("当前连接没有剪贴板权限，未执行粘贴")?;
                    let focus = self.focus.resolve().await;
                    let terminal = focus.as_ref().is_some_and(|f| f.terminal());
                    if terminal && shift.is_none() {
                        return Err("当前键盘布局无法执行终端粘贴，未执行输入".into());
                    }
                    let temporary = clipboard.begin(text).await?;
                    if let Some(focus) = &focus
                        && !focus.current().await
                    {
                        return Err("输入焦点已变化，未执行粘贴，请重新输入".into());
                    }
                    self.drain(state)?;
                    if !state.active()
                        || state.input_generation.load(Ordering::Acquire) != prepared.generation
                    {
                        return Err("粘贴前键盘配置已变化，未执行粘贴".into());
                    }
                    temporary.arm()?;
                    let mut keys = vec![(ctrl, true)];
                    if terminal && let Some(shift) = shift {
                        keys.push((shift, true));
                    }
                    keys.extend([(v, true), (v, false)]);
                    if terminal && let Some(shift) = shift {
                        keys.push((shift, false));
                    }
                    keys.push((ctrl, false));
                    let actions = keys
                        .into_iter()
                        .map(|(code, down)| Action {
                            device: d.clone(),
                            kind: Kind::Key(code, true, down),
                        })
                        .collect();
                    // Reuse all native effect fences and feedback synchronization.
                    Box::pin(self.apply_with_clipboard(
                        state,
                        Prepared {
                            generation: prepared.generation,
                            actions,
                            held: self.held.clone(),
                        },
                        None,
                    ))
                    .await?;
                    tokio::time::timeout(std::time::Duration::from_secs(1), temporary.delivered())
                        .await
                        .map_err(|_| "未确认粘贴数据交付，结果未知，禁止重放")??;
                    drop(temporary);
                    continue;
                }
                Kind::Synchronize => {
                    self.synchronize(state).await?;
                    if !state.active()
                        || state.input_generation.load(Ordering::Acquire) != prepared.generation
                    {
                        return Err(
                            "快捷键后的键盘配置发生变化，后续文字未执行，禁止重放整批输入".into(),
                        );
                    }
                    continue;
                }
                Kind::Move(x, y) => d
                    .interface::<ei::PointerAbsolute>()
                    .ok_or("鼠标设备已移除")?
                    .motion_absolute(x, y),
                Kind::Button(code, down) => {
                    d.interface::<ei::Button>().ok_or("鼠标按钮已移除")?.button(
                        code,
                        if down {
                            ei::button::ButtonState::Press
                        } else {
                            ei::button::ButtonState::Released
                        },
                    )
                }
                Kind::Key(code, physical, down) => {
                    let s = if down {
                        ei::keyboard::KeyState::Press
                    } else {
                        ei::keyboard::KeyState::Released
                    };
                    if physical {
                        if let Some(d) = self.devices.iter_mut().find(|v| &v.device == d)
                            && let Some(map) = d.keyboard.as_mut()
                        {
                            map.note_key(code, down);
                        }
                        d.interface::<ei::Keyboard>()
                            .ok_or("键盘设备已移除")?
                            .key(code, s);
                    } else {
                        d.interface::<ei::Text>()
                            .ok_or("文字设备已移除")?
                            .keysym(code, s);
                    }
                }
                Kind::Scroll(x, y) => d
                    .interface::<ei::Scroll>()
                    .ok_or("滚轮设备已移除")?
                    .scroll_discrete(x, y),
                Kind::Text(text) => d
                    .interface::<ei::Text>()
                    .ok_or("文字设备已移除")?
                    .utf8(&text),
            }
            // A press/release (or UTF-8 chunk) always gets its own frame. The
            // timestamp is CLOCK_MONOTONIC microseconds, not wall time/sequence.
            d.device().frame(self.connection.serial(), monotonic_us()?);
            self.flush()?;
        }
        self.synchronize(state).await?;
        if !state.active() || state.input_generation.load(Ordering::Acquire) != prepared.generation
        {
            return Err("输入确认期间授权或设备配置已变化，已结束远控，禁止重放".into());
        }
        self.held = prepared.held;
        Ok(())
    }
    pub(super) async fn synchronize(&mut self, state: &State) -> Result<()> {
        let callback = self.connection.connection().sync(1);
        let (tx, mut rx) = tokio::sync::oneshot::channel();
        self.converter.add_callback_handler(callback, move |_| {
            let _ = tx.send(());
        });
        self.flush()?;
        loop {
            tokio::select! {
                biased;
                result = &mut rx => {
                    result.map_err(|_| "Wayland EIS 同步中断")?;
                    for d in &mut self.devices { if let Some(map) = d.keyboard.as_mut() { map.synchronized(); } }
                    return Ok(());
                },
                event = self.next() => { self.handle(state, event)?; },
            }
        }
    }
    pub(super) async fn release(&mut self, state: &State) -> Result<()> {
        self.drain(state)?;
        let mut actions = Vec::new();
        for (code, device) in &self.held.buttons {
            actions.push(Action {
                device: device.clone(),
                kind: Kind::Button(*code as u32, false),
            });
        }
        for key in self.held.keys.values() {
            actions.push(Action {
                device: key.device.clone(),
                kind: Kind::Key(key.code, key.physical, false),
            });
        }
        self.apply(
            state,
            Prepared {
                generation: state.input_generation.load(Ordering::Acquire),
                actions,
                held: Held::default(),
            },
        )
        .await
    }
}
fn directions(direction: &str) -> Result<Vec<bool>> {
    match direction {
        "press" => Ok(vec![true]),
        "release" => Ok(vec![false]),
        "click" => Ok(vec![true, false]),
        _ => Err("输入方向无效".into()),
    }
}
fn validate_region(r: &reis::event::Region) -> Result<()> {
    // Preserve single-pixel accuracy when converting the protocol's u32 into f32.
    if r.width == 0
        || r.height == 0
        || r.width > 32768
        || r.height > 32768
        || r.x.checked_add(r.width).is_none_or(|v| v > 1 << 23)
        || r.y.checked_add(r.height).is_none_or(|v| v > 1 << 23)
        || !r.scale.is_finite()
        || r.scale <= 0.
    {
        return Err("Wayland 输入区域坐标无效".into());
    }
    Ok(())
}
fn text_chunks(mut text: &str) -> Vec<&str> {
    let mut chunks = Vec::new();
    while !text.is_empty() {
        let mut end = text.len().min(254);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        chunks.push(&text[..end]);
        text = &text[end..];
    }
    chunks
}
fn monotonic_us() -> Result<u64> {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: the pointer refers to a writable, initialized timespec.
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut time) } != 0 || time.tv_sec < 0 {
        return Err("无法读取 Wayland 输入时钟".into());
    }
    Ok((time.tv_sec as u64).saturating_mul(1_000_000) + time.tv_nsec as u64 / 1000)
}
