use super::{Command, Phase, State, StreamInfo, TokenStore, input::Event};
use crate::{Display, Result};
use futures::StreamExt;
use std::{
    collections::{HashMap, HashSet},
    fs::{self, OpenOptions},
    io::{Read, Write},
    sync::{Arc, atomic::Ordering},
    time::Duration,
};
use zbus::{
    Connection, Proxy,
    zvariant::{DeserializeDict, OwnedFd, OwnedObjectPath, Type, Value},
};
pub(super) const DEST: &str = "org.freedesktop.portal.Desktop";
pub(super) const ROOT: &str = "/org/freedesktop/portal/desktop";
pub(super) const CAST: &str = "org.freedesktop.portal.ScreenCast";
pub(super) const REMOTE: &str = "org.freedesktop.portal.RemoteDesktop";
pub(super) const SESSION: &str = "org.freedesktop.portal.Session";
#[derive(DeserializeDict, Type, Default)]
#[zvariant(signature = "dict")]
struct Reply {
    session_handle: Option<String>,
    streams: Option<Vec<(u32, Properties)>>,
    devices: Option<u32>,
    restore_token: Option<String>,
    clipboard_enabled: Option<bool>,
}
#[derive(DeserializeDict, Type)]
#[zvariant(signature = "dict")]
struct Properties {
    id: Option<String>,
    mapping_id: Option<String>,
    size: Option<(i32, i32)>,
    logical_size: Option<(i32, i32)>,
    position: Option<(i32, i32)>,
    source_type: Option<u32>,
    #[zvariant(rename = "pipewire-serial")]
    serial: Option<u64>,
}
fn token() -> String {
    format!("dogcodes_{}", uuid::Uuid::new_v4().simple())
}
fn options() -> HashMap<&'static str, Value<'static>> {
    HashMap::new()
}
async fn proxy(
    connection: &Connection,
    interface: &'static str,
    path: OwnedObjectPath,
) -> Result<Proxy<'static>> {
    Proxy::new(connection, DEST, path, interface)
        .await
        .map_err(|_| "Wayland 桌面门户不可用，请检查 xdg-desktop-portal 及桌面后端".into())
}
async fn root(connection: &Connection, interface: &'static str) -> Result<Proxy<'static>> {
    proxy(
        connection,
        interface,
        OwnedObjectPath::try_from(ROOT).unwrap(),
    )
    .await
}
async fn request<T: serde::Serialize + zbus::zvariant::DynamicType>(
    connection: &Connection,
    service: &Proxy<'_>,
    method: &str,
    handle: &str,
    body: &T,
) -> Result<Reply> {
    let unique = connection
        .unique_name()
        .ok_or("Wayland 消息总线身份不可用")?
        .as_str()
        .trim_start_matches(':')
        .replace('.', "_");
    let path = OwnedObjectPath::try_from(format!("{ROOT}/request/{unique}/{handle}"))
        .map_err(|_| "Wayland 请求身份无效")?;
    let receiver = proxy(connection, "org.freedesktop.portal.Request", path.clone()).await?;
    let mut replies = receiver
        .receive_signal("Response")
        .await
        .map_err(|_| "无法监听 Wayland 授权结果")?;
    let returned: OwnedObjectPath = service
        .call(method, body)
        .await
        .map_err(|_| format!("Wayland {method} 接口不可用，请检查系统桌面权限"))?;
    if returned != path {
        return Err("Wayland 授权回复身份不匹配".into());
    }
    let signal = replies.next().await.ok_or("Wayland 授权连接中断")?;
    let (code, response): (u32, Reply) = signal
        .body()
        .deserialize()
        .map_err(|_| "Wayland 授权回复无效")?;
    match code {
        0 => Ok(response),
        1 => Err("已在电脑上取消屏幕共享授权，可重新连接后重试".into()),
        _ => Err("Wayland 屏幕共享授权失败，请检查桌面门户".into()),
    }
}
struct Portal {
    connection: Connection,
    path: OwnedObjectPath,
    streams: Vec<StreamInfo>,
    cursor: u32,
    control: bool,
    closed: Option<zbus::proxy::SignalStream<'static>>,
    eis: Option<super::eis::Sender>,
    clipboard: Option<super::clipboard::Clipboard>,
}
impl Portal {
    async fn notify(&self, event: &Event) -> Result<()> {
        if self.eis.is_some() {
            return Err("EIS 输入连接禁止混用 D-Bus 输入".into());
        }
        let service = root(&self.connection, REMOTE).await?;
        let empty = options();
        let result = match event {
            Event::Move { stream, x, y } => {
                service
                    .call::<_, _, ()>(
                        "NotifyPointerMotionAbsolute",
                        &(&self.path, &empty, *stream, *x, *y),
                    )
                    .await
            }
            Event::Button(code, state) => {
                service
                    .call::<_, _, ()>("NotifyPointerButton", &(&self.path, &empty, *code, *state))
                    .await
            }
            Event::Key(code, state) => {
                service
                    .call::<_, _, ()>("NotifyKeyboardKeysym", &(&self.path, &empty, *code, *state))
                    .await
            }
            Event::Scroll(axis, steps) => {
                service
                    .call::<_, _, ()>(
                        "NotifyPointerAxisDiscrete",
                        &(&self.path, &empty, *axis, *steps),
                    )
                    .await
            }
        };
        result.map_err(|_| "Wayland 键鼠接口中断，已结束本次远控".into())
    }
    async fn close(&self) {
        if let Ok(service) = proxy(&self.connection, SESSION, self.path.clone()).await {
            let _ = tokio::time::timeout(
                Duration::from_secs(1),
                service.call::<_, _, ()>("Close", &()),
            )
            .await;
        }
    }
}
fn token_path(store: &TokenStore, control: bool) -> std::path::PathBuf {
    store.directory.join(if control {
        "control-token"
    } else {
        "view-token"
    })
}
fn take_token(store: &TokenStore, generation: u64, control: bool) -> Result<Option<String>> {
    let active = store.generation.lock().unwrap_or_else(|e| e.into_inner());
    if *active != generation {
        return Err("Wayland 授权已被替换".into());
    }
    let path = token_path(store, control);
    let mut file = match fs::File::open(&path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("无法读取 Wayland 授权恢复信息".into()),
    };
    let mut value = String::new();
    std::io::Read::by_ref(&mut file)
        .take(4097)
        .read_to_string(&mut value)
        .map_err(|_| "Wayland 授权恢复信息无效")?;
    // A restore token is single-use. Consume on disk before exposing it to D-Bus.
    fs::remove_file(path).map_err(|_| "无法更新 Wayland 授权恢复信息")?;
    fs::File::open(&store.directory)
        .and_then(|f| f.sync_all())
        .map_err(|_| "无法保存 Wayland 授权恢复状态")?;
    if value.len() > 4096 || value.is_empty() {
        return Err("Wayland 授权恢复信息无效".into());
    }
    Ok(Some(value))
}
fn save_token(
    store: &TokenStore,
    generation: u64,
    control: bool,
    value: &str,
    state: &State,
) -> Result<()> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let active = store.generation.lock().unwrap_or_else(|e| e.into_inner());
    if *active != generation || !state.active() {
        return Ok(());
    }
    if value.is_empty() || value.len() > 4096 {
        return Err("Wayland 授权恢复信息无效".into());
    }
    fs::create_dir_all(&store.directory).map_err(|_| "无法保存 Wayland 授权恢复信息")?;
    fs::set_permissions(&store.directory, fs::Permissions::from_mode(0o700))
        .map_err(|_| "无法保护 Wayland 授权恢复信息")?;
    let temporary = store.directory.join(token());
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(|_| "无法保存 Wayland 授权恢复信息")?;
        file.write_all(value.as_bytes())
            .and_then(|()| file.sync_all())
            .map_err(|_| "无法保存 Wayland 授权恢复信息")?;
        fs::rename(&temporary, token_path(store, control))
            .map_err(|_| "无法更新 Wayland 授权恢复信息")?;
        fs::File::open(&store.directory)
            .and_then(|f| f.sync_all())
            .map_err(|_| "无法保存 Wayland 授权恢复状态")?;
        Ok(())
    })();
    let _ = fs::remove_file(&temporary);
    result
}
async fn authorize(
    state: &Arc<State>,
    store: &TokenStore,
    generation: u64,
    control: bool,
    fresh: bool,
) -> Result<Portal> {
    let connection = Connection::session()
        .await
        .map_err(|_| "无法连接 Wayland 用户消息总线")?;
    let cast = root(&connection, CAST).await?;
    let service = if control {
        root(&connection, REMOTE).await?
    } else {
        cast.clone()
    };
    let version: u32 = service
        .get_property("version")
        .await
        .map_err(|_| "Wayland 远控门户不可用")?;
    let persistent = version >= if control { 2 } else { 4 };
    let restore = if persistent {
        let token = take_token(store, generation, control)?;
        if fresh { None } else { token }
    } else {
        None
    };
    let session_token = token();
    let handle = token();
    let mut create = options();
    create.insert("handle_token", Value::from(handle.clone()));
    create.insert("session_handle_token", Value::from(session_token.clone()));
    let response = request(&connection, &service, "CreateSession", &handle, &(create,)).await?;
    let path = OwnedObjectPath::try_from(response.session_handle.ok_or("Wayland 会话身份缺失")?)
        .map_err(|_| "Wayland 会话身份无效")?;
    let unique = connection
        .unique_name()
        .unwrap()
        .as_str()
        .trim_start_matches(':')
        .replace('.', "_");
    if path.as_str() != format!("{ROOT}/session/{unique}/{session_token}") {
        return Err("Wayland 会话身份不匹配".into());
    }
    // Keep a guard on every failure after creation. This private bus is also
    // disconnected on cancellation, so even a late grant cannot leave a session.
    let mut portal = Portal {
        connection,
        path,
        streams: Vec::new(),
        cursor: 1,
        control,
        closed: None,
        eis: None,
        clipboard: None,
    };
    let session_proxy = proxy(&portal.connection, SESSION, portal.path.clone()).await?;
    let mut closed = session_proxy
        .receive_signal("Closed")
        .await
        .map_err(|_| "无法监听 Wayland 授权撤销")?;
    let result = tokio::select! {
        biased;
        _=closed.next()=>Err("电脑已结束屏幕共享授权".into()),
        result=authorize_session(&mut portal, &cast, &service, persistent, restore, state)=>result,
    };
    portal.closed = Some(closed);
    match result {
        Ok(restore) => {
            if let Some(value) = restore
                && let Err(error) = save_token(store, generation, control, &value, state)
            {
                portal.close().await;
                return Err(error);
            }
            Ok(portal)
        }
        Err(error) => {
            portal.close().await;
            Err(error)
        }
    }
}
async fn authorize_session(
    portal: &mut Portal,
    cast: &Proxy<'_>,
    service: &Proxy<'_>,
    persistent: bool,
    restore: Option<String>,
    state: &Arc<State>,
) -> Result<Option<String>> {
    let available: u32 = cast.get_property("AvailableCursorModes").await.unwrap_or(1);
    portal.cursor = if available & 2 != 0 {
        2
    } else if available & 4 != 0 {
        4
    } else {
        1
    };
    if portal.control {
        let handle = token();
        let mut devices = options();
        devices.insert("handle_token", Value::from(handle.clone()));
        devices.insert("types", Value::from(3u32));
        if persistent {
            devices.insert("persist_mode", Value::from(2u32));
            if let Some(value) = restore.as_ref() {
                devices.insert("restore_token", Value::from(value.clone()));
            }
        }
        request(
            &portal.connection,
            service,
            "SelectDevices",
            &handle,
            &(&portal.path, devices),
        )
        .await?;
    }
    let handle = token();
    let mut sources = options();
    sources.insert("handle_token", Value::from(handle.clone()));
    sources.insert("types", Value::from(1u32));
    sources.insert("multiple", Value::from(true));
    sources.insert("cursor_mode", Value::from(portal.cursor));
    // Combined sessions persist exclusively through RemoteDesktop.SelectDevices.
    if !portal.control && persistent {
        sources.insert("persist_mode", Value::from(2u32));
        if let Some(value) = restore {
            sources.insert("restore_token", Value::from(value));
        }
    }
    request(
        &portal.connection,
        cast,
        "SelectSources",
        &handle,
        &(&portal.path, sources),
    )
    .await?;
    if portal.control {
        portal.clipboard =
            super::clipboard::Clipboard::request(&portal.connection, &portal.path, state.clone())
                .await;
    }
    let handle = token();
    let mut start = options();
    start.insert("handle_token", Value::from(handle.clone()));
    let reply = request(
        &portal.connection,
        service,
        "Start",
        &handle,
        &(&portal.path, "", start),
    )
    .await?;
    if !reply.clipboard_enabled.unwrap_or(false) {
        portal.clipboard = None;
    }
    if portal.control && reply.devices.unwrap_or(0) & 3 != 3 {
        return Err("电脑没有授权完整键鼠权限，请重新连接并允许".into());
    }
    let streams = reply.streams.ok_or("Wayland 没有返回授权屏幕")?;
    if streams.is_empty() || streams.len() > 16 {
        return Err("Wayland 授权屏幕数量无效".into());
    }
    let mut nodes = HashSet::new();
    let mut mappings = HashSet::new();
    for (index, (node, props)) in streams.into_iter().enumerate() {
        if !nodes.insert(node) || props.source_type.is_some_and(|v| v != 1) {
            return Err("Wayland 返回的屏幕来源不匹配".into());
        }
        let (width, height) = props
            .logical_size
            .or(props.size)
            .ok_or("Wayland 未提供屏幕逻辑尺寸，无法可靠映射键鼠")?;
        if width <= 0 || height <= 0 || width > 32768 || height > 32768 {
            return Err("Wayland 屏幕逻辑尺寸无效".into());
        }
        let (x, y) = props.position.unwrap_or((0, 0));
        if let Some(mapping) = props.mapping_id.as_ref()
            && (mapping.is_empty() || mapping.len() > 1024 || !mappings.insert(mapping.clone()))
        {
            return Err("Wayland 屏幕输入映射身份无效或重复".into());
        }
        portal.streams.push(StreamInfo {
            node,
            serial: props.serial,
            mapping_id: props.mapping_id,
            display: Display {
                id: index as u32 + 1,
                name: props
                    .id
                    .filter(|s| !s.is_empty() && s.len() < 128)
                    .unwrap_or_else(|| format!("屏幕 {}", index + 1)),
                x,
                y,
                width: width as u32,
                height: height as u32,
                scale: 1.,
                primary: index == 0,
            },
        });
    }
    // Select a backend before sending any input. An older portal without stable
    // mapping IDs keeps Notify. After receiving an EIS FD there is no fallback:
    // handshake/EOF/errors must end this same authorized session.
    if portal.control && persistent && portal.streams.iter().all(|s| s.mapping_id.is_some()) {
        let fd: OwnedFd = service
            .call("ConnectToEIS", &(&portal.path, options()))
            .await
            .map_err(|_| "无法连接 Wayland EIS 输入接口，请重新连接并授权")?;
        portal.eis = Some(
            tokio::time::timeout(Duration::from_secs(10), super::eis::Sender::open(fd.into()))
                .await
                .map_err(|_| "Wayland EIS 输入握手超时，请重新连接")??,
        );
        tokio::time::timeout(
            Duration::from_secs(10),
            portal
                .eis
                .as_mut()
                .unwrap()
                .initialize(state, &portal.streams),
        )
        .await
        .map_err(|_| "Wayland 输入设备未就绪，请重新连接并授权")??;
    }
    Ok(reply.restore_token)
}
async fn cancelled(state: &State) {
    while state.active() {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
pub(super) async fn run(
    state: Arc<State>,
    mut commands: tokio::sync::mpsc::Receiver<Command>,
    store: Arc<TokenStore>,
    generation: u64,
    control: bool,
    fresh: bool,
) {
    let mut portal = tokio::select! {
        biased;
     _=cancelled(&state)=>return,
     result=tokio::time::timeout(Duration::from_secs(120),authorize(&state,&store,generation,control,fresh))=>match result {
      Ok(Ok(portal))=>portal,
      Ok(Err(error))=>{state.fail(error);return;},
      Err(_)=>{state.fail("等待电脑屏幕共享授权超时，请重新连接后重试".into());return;}
     }
    };
    state
        .eis_active
        .store(portal.eis.is_some(), Ordering::Release);
    let Some(mut closed) = portal.closed.take() else {
        state.fail("无法监听 Wayland 授权撤销".into());
        portal.close().await;
        return;
    };
    *state.phase.lock().unwrap_or_else(|e| e.into_inner()) =
        Phase::Ready(portal.streams.clone(), portal.cursor);
    let mut held = Held::default();
    loop {
        tokio::select! {
            biased;
            _ = cancelled(&state) => break,
            _ = closed.next() => {
                state.fail("电脑已结束屏幕共享，请重新连接并授权".into());
                break;
            },
            event = async {
                match portal.eis.as_mut() {
                    Some(eis) => eis.next().await,
                    None => std::future::pending().await,
                }
            } => {
                if let Some(eis) = portal.eis.as_mut()
                    && let Err(error) = eis.handle(&state, event) {
                    state.fail(error);
                    break;
                }
            },
            command = commands.recv() => {
                let Some(command) = command else { break; };
                let result = tokio::select! {
                    biased;
                    _ = cancelled(&state) => break,
                    _ = closed.next() => {
                        state.fail("电脑已结束屏幕共享，请重新连接并授权".into());
                        break;
                    },
                    result = tokio::time::timeout(Duration::from_secs(3), execute(&mut portal, &state, &mut held, command)) => result,
                };
                match result {
                    Ok(Ok(())) => {},
                    Ok(Err(error)) => { state.fail(error); break; },
                    Err(_) => {
                        state.fail("Wayland 系统操作超时，已结束本次远控，禁止重放".into());
                        break;
                    },
                }
            },
        }
    }
    state.closed.store(true, Ordering::Release);
    portal.close().await;
}

#[derive(Default)]
struct Held {
    keys: HashSet<i32>,
    buttons: HashSet<i32>,
}
async fn execute(
    portal: &mut Portal,
    state: &State,
    held: &mut Held,
    command: Command,
) -> Result<()> {
    match command {
        Command::Open { display, reply } => {
            let result = if portal
                .streams
                .iter()
                .any(|stream| stream.display.id == display)
            {
                match root(&portal.connection, CAST).await {
                    Ok(cast) => cast
                        .call::<_, _, OwnedFd>("OpenPipeWireRemote", &(&portal.path, options()))
                        .await
                        .map(std::os::fd::OwnedFd::from)
                        .map_err(|_| "Wayland 已授权，但无法连接屏幕流".into()),
                    Err(error) => Err(error),
                }
            } else {
                Err("屏幕不在授权范围内".into())
            };
            let _ = reply.send(result);
            Ok(())
        }
        Command::Input {
            stream,
            generation,
            raw,
            events,
            reply,
        } => {
            if let Some(eis) = portal.eis.as_mut() {
                eis.synchronize(state).await?;
                let prepared = match eis.prepare_with_clipboard(
                    state,
                    &stream,
                    generation,
                    raw,
                    portal.clipboard.is_some(),
                ) {
                    Ok(prepared) => prepared,
                    Err(error) => {
                        let _ = reply.send(Err(error));
                        return Ok(());
                    }
                };
                let result = eis
                    .apply_with_clipboard(state, prepared, portal.clipboard.as_ref())
                    .await;
                let failed = result.is_err();
                let _ = reply.send(result);
                return if failed {
                    Err("Wayland EIS 输入中断，已结束本次远控，禁止重放".into())
                } else {
                    Ok(())
                };
            }
            // Recheck queued input at the actor boundary before any OS effect.
            // Rejecting a complete batch here leaves the viewing grant intact.
            if let Err(error) = state.check_input_geometry(&events) {
                let _ = reply.send(Err(error));
                return Ok(());
            }
            let result = async {
                if !portal.control {
                    return Err("当前连接为仅查看模式".into());
                }
                for event in events {
                    if !state.active() {
                        return Err("远控已结束，禁止执行旧输入".into());
                    }
                    state.check_input_geometry(std::slice::from_ref(&event))?;
                    portal.notify(&event).await?;
                    match event {
                        Event::Key(code, 1) => {
                            held.keys.insert(code);
                        }
                        Event::Key(code, 0) => {
                            held.keys.remove(&code);
                        }
                        Event::Button(code, 1) => {
                            held.buttons.insert(code);
                        }
                        Event::Button(code, 0) => {
                            held.buttons.remove(&code);
                        }
                        _ => {}
                    }
                }
                Ok(())
            }
            .await;
            let failed = result.is_err();
            let _ = reply.send(result);
            if failed {
                Err("Wayland 键鼠操作中断，已结束本次远控".into())
            } else {
                Ok(())
            }
        }
        Command::Release => {
            if let Some(eis) = portal.eis.as_mut() {
                return eis.release(state).await;
            }
            for code in held.buttons.drain() {
                portal.notify(&Event::Button(code, 0)).await?;
            }
            for code in held.keys.drain() {
                portal.notify(&Event::Key(code, 0)).await?;
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    fn state() -> State {
        State {
            phase: std::sync::Mutex::new(Phase::Pending),
            closed: std::sync::atomic::AtomicBool::new(false),
            input_geometry_changed: std::sync::atomic::AtomicBool::new(false),
            eis_active: std::sync::atomic::AtomicBool::new(false),
            input_generation: std::sync::atomic::AtomicU64::new(0),
            alive: Arc::new(|| true),
        }
    }
    #[test]
    fn changed_pixels_never_redefine_logical_pointer_space_but_releases_remain_available() {
        let state = state();
        let position = [Event::Move {
            stream: 7,
            x: 100.,
            y: 200.,
        }];
        assert!(state.check_input_geometry(&position).is_ok());
        state.input_geometry_changed.store(true, Ordering::Release);
        for events in [
            position.to_vec(),
            vec![Event::Button(0x110, 1)],
            vec![Event::Scroll(0, 5)],
            vec![
                Event::Key(32, 1),
                Event::Move {
                    stream: 7,
                    x: 1.,
                    y: 2.,
                },
            ],
        ] {
            assert!(state.check_input_geometry(&events).is_err());
        }
        assert!(
            state
                .check_input_geometry(&[
                    Event::Button(0x110, 0),
                    Event::Key(32, 0),
                    Event::Key(32, 1)
                ])
                .is_ok()
        );
        assert!(
            state.active(),
            "viewing grant was revoked by geometry alone"
        );
        assert!(state.check_input_geometry(&position).is_err());
    }
    #[test]
    fn restore_tokens_are_private_consumed_once_and_cannot_be_overwritten_by_old_sessions() {
        let directory = tempfile::tempdir().unwrap();
        let store = TokenStore::new(directory.path().join("portal"));
        let state = state();
        *store.generation.lock().unwrap() = 1;
        save_token(&store, 1, true, "token-one", &state).unwrap();
        assert_eq!(
            fs::metadata(token_path(&store, true))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(&store.directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(take_token(&store, 1, false).unwrap(), None);
        assert_eq!(
            take_token(&store, 1, true).unwrap().as_deref(),
            Some("token-one")
        );
        assert_eq!(take_token(&store, 1, true).unwrap(), None);
        *store.generation.lock().unwrap() = 2;
        save_token(&store, 2, true, "token-new", &state).unwrap();
        save_token(&store, 1, true, "token-stale", &state).unwrap();
        assert!(take_token(&store, 1, true).is_err());
        state.closed.store(true, Ordering::Release);
        save_token(&store, 2, true, "token-cancelled", &state).unwrap();
        assert_eq!(
            take_token(&store, 2, true).unwrap().as_deref(),
            Some("token-new")
        );
    }
}
