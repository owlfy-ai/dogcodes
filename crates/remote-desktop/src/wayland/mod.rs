//! One connection-owned portal session for capture and input. The private
//! D-Bus connection, restore-token rotation and input queue belong to its actor.
mod binding;
mod capture;
mod clipboard;
mod cursor;
mod eis;
mod focus;
mod geometry;
mod gpu;
mod input;
mod loan;
mod memory;
mod metadata;
mod pixels;
mod portal;
use crate::{Display, Input, Result};
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::mpsc;

pub fn available() -> bool {
    std::env::var("XDG_SESSION_TYPE").is_ok_and(|s| s.eq_ignore_ascii_case("wayland"))
        || std::env::var("WAYLAND_DISPLAY").is_ok_and(|s| !s.is_empty())
}
#[derive(Clone)]
pub(super) struct StreamInfo {
    pub display: Display,
    pub node: u32,
    pub serial: Option<u64>,
    pub mapping_id: Option<String>,
}
#[derive(Clone)]
enum Phase {
    Pending,
    Ready(Vec<StreamInfo>, u32),
    Error(String),
}
struct State {
    phase: Mutex<Phase>,
    closed: AtomicBool,
    input_geometry_changed: AtomicBool,
    eis_active: AtomicBool,
    input_generation: AtomicU64,
    alive: Arc<dyn Fn() -> bool + Send + Sync>,
}
impl State {
    fn check_input_geometry(&self, events: &[input::Event]) -> Result<()> {
        if !self.eis_active.load(Ordering::Acquire)
            && self.input_geometry_changed.load(Ordering::Acquire)
            && events.iter().any(|event| {
                matches!(
                    event,
                    input::Event::Move { .. }
                        | input::Event::Button(_, 1)
                        | input::Event::Scroll(..)
                )
            })
        {
            return Err("屏幕尺寸已变化，鼠标坐标需要重新授权，请重新连接远程桌面".into());
        }
        Ok(())
    }
    fn active(&self) -> bool {
        !self.closed.load(Ordering::Acquire) && (self.alive)()
    }
    fn fail(&self, error: String) {
        *self.phase.lock().unwrap_or_else(|e| e.into_inner()) = Phase::Error(error);
    }
}
pub struct TokenStore {
    directory: PathBuf,
    generation: Mutex<u64>,
}
impl TokenStore {
    pub fn new(directory: PathBuf) -> Self {
        Self {
            directory,
            generation: Mutex::new(0),
        }
    }
}
static ACTORS: AtomicUsize = AtomicUsize::new(0);
struct Slot;
impl Slot {
    fn take() -> Result<Self> {
        ACTORS
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < 2).then_some(n + 1)
            })
            .map(|_| Self)
            .map_err(|_| "旧 Wayland 授权仍在结束，请稍后重试".into())
    }
}
impl Drop for Slot {
    fn drop(&mut self) {
        ACTORS.fetch_sub(1, Ordering::AcqRel);
    }
}
enum Command {
    Open {
        display: u32,
        reply: std::sync::mpsc::SyncSender<Result<std::os::fd::OwnedFd>>,
    },
    Input {
        stream: StreamInfo,
        generation: u64,
        raw: Vec<Input>,
        events: Vec<input::Event>,
        reply: std::sync::mpsc::SyncSender<Result<()>>,
    },
    Release,
}
struct Client {
    state: Arc<State>,
    sender: mpsc::Sender<Command>,
}
impl Drop for Client {
    fn drop(&mut self) {
        self.state.closed.store(true, Ordering::Release);
    }
}
#[derive(Clone)]
pub struct Session {
    inner: Arc<Client>,
}
impl Session {
    pub fn start(store: Arc<TokenStore>, control: bool) -> Result<Self> {
        Self::start_owned(store, control, false, Arc::new(|| true))
    }
    pub fn start_owned(
        store: Arc<TokenStore>,
        control: bool,
        fresh: bool,
        alive: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Result<Self> {
        let slot = Slot::take()?;
        let generation = {
            let mut n = store.generation.lock().unwrap_or_else(|e| e.into_inner());
            *n = n.checked_add(1).ok_or("授权次数已耗尽，请重启应用")?;
            *n
        };
        let state = Arc::new(State {
            phase: Mutex::new(Phase::Pending),
            closed: AtomicBool::new(false),
            input_geometry_changed: AtomicBool::new(false),
            eis_active: AtomicBool::new(false),
            input_generation: AtomicU64::new(0),
            alive,
        });
        let (sender, receiver) = mpsc::channel(16);
        let shared = state.clone();
        std::thread::Builder::new()
            .name("wayland-portal".into())
            .spawn(move || {
                let _slot = slot;
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(r) => r,
                    Err(_) => {
                        shared.fail("无法启动 Wayland 授权线程".into());
                        return;
                    }
                };
                runtime.block_on(portal::run(
                    shared, receiver, store, generation, control, fresh,
                ));
            })
            .map_err(|_| "无法启动 Wayland 授权线程")?;
        Ok(Self {
            inner: Arc::new(Client { state, sender }),
        })
    }
    pub fn status(&self) -> Result<bool> {
        let phase = self
            .inner
            .state
            .phase
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Phase::Error(error) = &*phase {
            return Err(error.clone());
        }
        if !self.inner.state.active() {
            return Err("Wayland 授权会话已结束".into());
        }
        Ok(matches!(&*phase, Phase::Ready(..)))
    }
    fn streams(&self) -> Result<Vec<StreamInfo>> {
        if !self.status()? {
            return Err("请在电脑上完成 Wayland 屏幕共享授权".into());
        }
        match &*self
            .inner
            .state
            .phase
            .lock()
            .unwrap_or_else(|e| e.into_inner())
        {
            Phase::Ready(streams, _) => Ok(streams.clone()),
            _ => Err("Wayland 授权会话已结束".into()),
        }
    }
    pub fn displays(&self) -> Result<Vec<Display>> {
        Ok(self.streams()?.into_iter().map(|s| s.display).collect())
    }
    fn stream(&self, id: u32) -> Result<StreamInfo> {
        self.streams()?
            .into_iter()
            .find(|s| s.display.id == id)
            .ok_or_else(|| "该屏幕不在本次 Wayland 授权中".into())
    }
    fn cursor_mode(&self) -> u32 {
        match &*self
            .inner
            .state
            .phase
            .lock()
            .unwrap_or_else(|e| e.into_inner())
        {
            Phase::Ready(_, mode) => *mode,
            _ => 1,
        }
    }
    pub fn cursor_embedded(&self) -> bool {
        matches!(self.cursor_mode(), 2 | 4)
    }
    pub fn close(&self) {
        self.inner.state.closed.store(true, Ordering::Release);
    }
    fn invalidate_input_geometry(&self) {
        // Pixel changes cannot supply the portal's logical_size. Keep viewing
        // and releases/keyboard available; a fresh grant restores pointer input.
        self.inner
            .state
            .input_geometry_changed
            .store(true, Ordering::Release);
    }
    pub fn capture(&self, id: u32) -> Result<Arc<dyn crate::video::ScreenSource>> {
        let stream = self.stream(id)?;
        let (reply, rx) = std::sync::mpsc::sync_channel(1);
        self.inner
            .sender
            .try_send(Command::Open { display: id, reply })
            .map_err(|_| "Wayland 系统接口繁忙")?;
        let fd = rx
            .recv_timeout(Duration::from_secs(4))
            .map_err(|_| "Wayland 屏幕连接超时")??;
        capture::start(self.clone(), stream, fd)
    }
    pub fn send(&self, id: u32, events: Vec<Input>) -> Result<()> {
        let generation = self.inner.state.input_generation.load(Ordering::Acquire);
        let stream = self.stream(id)?;
        let planned = input::plan(
            stream.node,
            (stream.display.width, stream.display.height),
            events.clone(),
        )?;
        self.inner.state.check_input_geometry(&planned)?;
        let (reply, rx) = std::sync::mpsc::sync_channel(1);
        self.inner
            .sender
            .try_send(Command::Input {
                stream,
                generation,
                raw: events,
                events: planned,
                reply,
            })
            .map_err(|_| "Wayland 键鼠队列繁忙")?;
        match rx.recv_timeout(Duration::from_secs(4)) {
            Ok(result) => result,
            Err(_) => {
                self.close();
                Err("Wayland 键鼠操作结果未知，已结束远控，禁止重放".into())
            }
        }
    }
    pub fn release_held(&self) {
        if self.inner.sender.try_send(Command::Release).is_err() {
            self.close();
        }
    }
}

#[cfg(test)]
mod fixture;
