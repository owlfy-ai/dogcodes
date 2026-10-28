//! Connection-owned loopback developer-service tunnel, including HMR `WebSockets`.
//! No listening port or public URL is created on the PC.
use super::{Arc, Core, Duration, Message, Mutex, Value, failure, json};
use base64::{Engine, engine::general_purpose::STANDARD};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap};
use std::time::Instant;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

pub(super) const METHODS: &[&str] = &[
    "preview_open",
    "preview_close",
    "preview_http",
    "preview_ws_open",
    "preview_ws_read",
    "preview_ws_write",
    "preview_ws_close",
];
const BODY: usize = 4 * 1024 * 1024;
const WS_FRAME: usize = 1024 * 1024;
type LocalSocket = tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>;
type Writer = futures_util::stream::SplitSink<LocalSocket, Message>;
type Reader = futures_util::stream::SplitStream<LocalSocket>;
struct Target {
    port: u16,
    task: String,
    touched: Instant,
}
struct Web {
    target: String,
    writer: Mutex<(Writer, u64)>,
    reader: Mutex<Reader>,
}
#[derive(Default)]
pub(super) struct Host {
    targets: Mutex<HashMap<String, Target>>,
    webs: Mutex<HashMap<String, Arc<Web>>>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Open {
    task_id: String,
    port: u16,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Http {
    preview_id: String,
    path: String,
    #[serde(default = "get_method")]
    method: String,
    #[serde(default)]
    headers: BTreeMap<String, String>,
    #[serde(default)]
    body: String,
}
fn get_method() -> String {
    "GET".into()
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WebOpen {
    preview_id: String,
    path: String,
    #[serde(default)]
    protocols: Vec<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WebWrite {
    socket_id: String,
    sequence: String,
    text: Option<String>,
    bytes: Option<String>,
}
fn path(path: &str) -> Result<(), String> {
    if !path.starts_with('/')
        || path.starts_with("//")
        || path.len() > 8192
        || path
            .chars()
            .any(|c| c.is_control() || c == '\\' || c == '#')
    {
        return Err("预览路径无效".into());
    }
    Ok(())
}
fn end_header(name: &str) -> bool {
    !matches!(
        name.to_ascii_lowercase().as_str(),
        "host"
            | "connection"
            | "upgrade"
            | "keep-alive"
            | "transfer-encoding"
            | "te"
            | "trailer"
            | "content-length"
            | "proxy-authorization"
            | "proxy-authenticate"
    )
}
// The WebView has a different loopback port on the phone. Same-service
// redirects must remain relative to that virtual origin, without following
// redirects here or turning another PC service into the selected target.
fn preview_location(value: &str, port: u16) -> String {
    let parsed = if value.starts_with("//") {
        reqwest::Url::parse(&format!("http:{value}"))
    } else {
        reqwest::Url::parse(value)
    };
    let Ok(target) = parsed else {
        return value.to_owned();
    };
    if target.scheme() != "http"
        || !matches!(target.host_str(), Some("127.0.0.1" | "localhost"))
        || target.port_or_known_default() != Some(port)
        || !target.username().is_empty()
        || target.password().is_some()
        || target.path().starts_with("//")
    {
        return value.to_owned();
    }
    let mut relative = target.path().to_owned();
    if let Some(query) = target.query() {
        relative.push('?');
        relative.push_str(query);
    }
    if let Some(fragment) = target.fragment() {
        relative.push('#');
        relative.push_str(fragment);
    }
    relative
}
impl Host {
    async fn target(&self, core: &Core, id: &str) -> Result<u16, String> {
        let (port, task) = {
            let mut targets = self.targets.lock().await;
            let target = targets.get_mut(id).ok_or("预览已结束，请重新打开")?;
            if target.touched.elapsed() > Duration::from_mins(5) {
                return Err("预览已过期，请重新打开".into());
            }
            target.touched = Instant::now();
            (target.port, target.task.clone())
        };
        crate::workspace_service::project_paths(core, &task).await?;
        Ok(port)
    }
    async fn web(&self, core: &Core, id: &str) -> Result<Arc<Web>, String> {
        let web = self
            .webs
            .lock()
            .await
            .get(id)
            .cloned()
            .ok_or("预览 WebSocket 已关闭")?;
        self.target(core, &web.target).await?;
        Ok(web)
    }
    pub(super) async fn call(
        &self,
        core: &Arc<Core>,
        method: &str,
        args: Value,
    ) -> Result<Value, String> {
        match method {
            "preview_open" => {
                let request: Open = serde_json::from_value(args).map_err(failure)?;
                if request.port == 0 {
                    return Err("预览端口无效".into());
                }
                crate::workspace_service::project_paths(core, &request.task_id).await?;
                let mut targets = self.targets.lock().await;
                targets.retain(|_, target| target.touched.elapsed() <= Duration::from_mins(5));
                let active = targets
                    .keys()
                    .cloned()
                    .collect::<std::collections::BTreeSet<_>>();
                self.webs
                    .lock()
                    .await
                    .retain(|_, web| active.contains(&web.target));
                if targets.len() >= 4 {
                    return Err("最多同时打开 4 个项目预览".into());
                }
                let id = uuid::Uuid::new_v4().to_string();
                targets.insert(
                    id.clone(),
                    Target {
                        port: request.port,
                        task: request.task_id,
                        touched: Instant::now(),
                    },
                );
                Ok(
                    json!({"previewId":id,"port":request.port,"idleTimeoutSeconds":300,"maxBodyBytes":BODY,"webSocket":true}),
                )
            }
            "preview_close" => {
                let id = args.as_str().ok_or("预览标识无效")?;
                let removed = self.targets.lock().await.remove(id).is_some();
                self.webs.lock().await.retain(|_, web| web.target != id);
                Ok(json!({"closed":removed}))
            }
            "preview_http" => {
                let request: Http = serde_json::from_value(args).map_err(failure)?;
                path(&request.path)?;
                let method =
                    reqwest::Method::from_bytes(request.method.as_bytes()).map_err(failure)?;
                if !matches!(
                    method,
                    reqwest::Method::GET
                        | reqwest::Method::HEAD
                        | reqwest::Method::POST
                        | reqwest::Method::PUT
                        | reqwest::Method::PATCH
                        | reqwest::Method::DELETE
                        | reqwest::Method::OPTIONS
                ) {
                    return Err("预览不支持此 HTTP 方法".into());
                }
                if request.body.len() > BODY.div_ceil(3) * 4 || request.headers.len() > 64 {
                    return Err("预览请求过大".into());
                }
                let body = STANDARD
                    .decode(&request.body)
                    .map_err(|_| "预览请求编码无效")?;
                if body.len() > BODY {
                    return Err("预览请求过大".into());
                }
                let port = self.target(core, &request.preview_id).await?;
                let origin = format!("http://127.0.0.1:{port}");
                let client = reqwest::Client::builder()
                    .no_proxy()
                    .retry(reqwest::retry::never())
                    .redirect(reqwest::redirect::Policy::none())
                    .timeout(Duration::from_secs(30))
                    .build()
                    .map_err(failure)?;
                let mut call = client
                    .request(method, format!("{origin}{}", request.path))
                    .body(body);
                for (name, value) in request.headers {
                    if !end_header(&name) || name.len() > 128 || value.len() > 8192 {
                        return Err("预览请求头无效".into());
                    }
                    // WebViews use a phone-local origin. Development servers expect their own.
                    let value = if name.eq_ignore_ascii_case("origin") {
                        origin.clone()
                    } else if name.eq_ignore_ascii_case("referer") {
                        format!("{origin}/")
                    } else {
                        value
                    };
                    call = call.header(name, value);
                }
                let mut response = call
                    .send()
                    .await
                    .map_err(|_| "无法访问项目服务，请确认项目运行及端口；未重发请求")?;
                let status = response.status().as_u16();
                let headers = response
                    .headers()
                    .iter()
                    .filter(|(name, _)| end_header(name.as_str()))
                    .filter_map(|(name, value)| {
                        value.to_str().ok().map(|value| {
                            (
                                name.as_str().to_string(),
                                if name == reqwest::header::LOCATION {
                                    preview_location(value, port)
                                } else {
                                    value.to_owned()
                                },
                            )
                        })
                    })
                    .collect::<Vec<_>>();
                if response
                    .content_length()
                    .is_some_and(|length| length > BODY as u64)
                {
                    return Err("预览响应超过 4 MB".into());
                }
                let mut bytes = Vec::new();
                while let Some(chunk) = response
                    .chunk()
                    .await
                    .map_err(|_| "项目响应中断，未自动重发请求")?
                {
                    if bytes.len().saturating_add(chunk.len()) > BODY {
                        return Err("预览响应超过 4 MB".into());
                    }
                    bytes.extend_from_slice(&chunk);
                }
                Ok(json!({"status":status,"headers":headers,"body":STANDARD.encode(bytes)}))
            }
            "preview_ws_open" => {
                let request: WebOpen = serde_json::from_value(args).map_err(failure)?;
                path(&request.path)?;
                if request.protocols.len() > 8
                    || request.protocols.iter().any(|p| {
                        p.is_empty()
                            || p.len() > 128
                            || !p.bytes().all(|b| {
                                b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b)
                            })
                    })
                {
                    return Err("WebSocket 子协议无效".into());
                }
                // Check again when inserting after the network handshake.
                let mut webs = self.webs.lock().await;
                if webs.len() >= 8 {
                    return Err("预览 WebSocket 数量超过限制".into());
                }
                // Do not acquire targets while holding webs (preview_open uses the opposite order).
                drop(webs);
                let port = self.target(core, &request.preview_id).await?;
                let mut call = format!("ws://127.0.0.1:{port}{}", request.path)
                    .into_client_request()
                    .map_err(failure)?;
                call.headers_mut().insert(
                    "Origin",
                    format!("http://127.0.0.1:{port}")
                        .parse()
                        .map_err(failure)?,
                );
                if !request.protocols.is_empty() {
                    call.headers_mut().insert(
                        "Sec-WebSocket-Protocol",
                        request.protocols.join(", ").parse().map_err(failure)?,
                    );
                }
                let stream = tokio::time::timeout(
                    Duration::from_secs(5),
                    tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port)),
                )
                .await
                .map_err(|_| "项目 WebSocket 连接超时")?
                .map_err(|_| "项目 WebSocket 未运行")?;
                let config = tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
                    .max_message_size(Some(WS_FRAME))
                    .max_frame_size(Some(WS_FRAME));
                let (socket, response) = tokio::time::timeout(
                    Duration::from_secs(5),
                    tokio_tungstenite::client_async_with_config(call, stream, Some(config)),
                )
                .await
                .map_err(|_| "项目 WebSocket 握手超时")?
                .map_err(|_| "项目 WebSocket 握手失败")?;
                let protocol = response
                    .headers()
                    .get("Sec-WebSocket-Protocol")
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string);
                let (writer, reader) = socket.split();
                let targets = self.targets.lock().await;
                if !targets.contains_key(&request.preview_id) {
                    return Err("预览已关闭".into());
                }
                webs = self.webs.lock().await;
                if webs.len() >= 8 {
                    return Err("预览 WebSocket 数量超过限制".into());
                }
                let id = uuid::Uuid::new_v4().to_string();
                webs.insert(
                    id.clone(),
                    Arc::new(Web {
                        target: request.preview_id,
                        writer: Mutex::new((writer, 0)),
                        reader: Mutex::new(reader),
                    }),
                );
                Ok(
                    json!({"socketId":id,"protocol":protocol,"sequence":"0","maxMessageBytes":WS_FRAME}),
                )
            }
            "preview_ws_read" => {
                let id = args.as_str().ok_or("WebSocket 标识无效")?;
                let web = self.web(core, id).await?;
                let mut reader = web
                    .reader
                    .try_lock()
                    .map_err(|_| "已有 WebSocket 读取在进行")?;
                match tokio::time::timeout(Duration::from_secs(5), reader.next()).await {
                    Ok(Some(Ok(Message::Text(text)))) => {
                        Ok(json!({"type":"text","text":text.as_str()}))
                    }
                    Ok(Some(Ok(Message::Binary(bytes)))) => {
                        Ok(json!({"type":"binary","bytes":STANDARD.encode(bytes)}))
                    }
                    Ok(Some(Ok(Message::Ping(_)))) => {
                        drop(reader);
                        let mut writer = web.writer.lock().await;
                        tokio::time::timeout(Duration::from_secs(5), writer.0.flush())
                            .await
                            .map_err(|_| "项目 WebSocket 已中断")?
                            .map_err(|_| "项目 WebSocket 已中断")?;
                        Ok(json!({"type":"pending"}))
                    }
                    Err(_) | Ok(Some(Ok(Message::Pong(_)))) => Ok(json!({"type":"pending"})),
                    _ => {
                        self.webs.lock().await.remove(id);
                        Ok(json!({"type":"closed"}))
                    }
                }
            }
            "preview_ws_write" => {
                let request: WebWrite = serde_json::from_value(args).map_err(failure)?;
                let web = self.web(core, &request.socket_id).await?;
                let message = match (request.text, request.bytes) {
                    (Some(text), None) if text.len() <= WS_FRAME => Message::Text(text.into()),
                    (None, Some(encoded)) if encoded.len() <= WS_FRAME.div_ceil(3) * 4 => {
                        let bytes = STANDARD.decode(encoded).map_err(|_| "WebSocket 编码无效")?;
                        if bytes.len() > WS_FRAME {
                            return Err("WebSocket 消息过大".into());
                        }
                        Message::Binary(bytes.into())
                    }
                    _ => return Err("WebSocket 消息无效".into()),
                };
                let mut writer = web.writer.lock().await;
                let sequence = request
                    .sequence
                    .parse::<u64>()
                    .map_err(|_| "WebSocket 序号无效")?;
                if Some(sequence) != writer.1.checked_add(1) {
                    return Err("WebSocket 序号冲突，未重发".into());
                }
                writer.1 = sequence;
                if !matches!(
                    tokio::time::timeout(Duration::from_secs(5), writer.0.send(message)).await,
                    Ok(Ok(()))
                ) {
                    self.webs.lock().await.remove(&request.socket_id);
                    return Err("项目 WebSocket 已中断，未重发消息".into());
                }
                Ok(json!({"sequence":sequence.to_string()}))
            }
            "preview_ws_close" => Ok(
                json!({"closed":self.webs.lock().await.remove(args.as_str().ok_or("WebSocket 标识无效")?).is_some()}),
            ),
            _ => Err("remote preview operation unavailable".into()),
        }
    }
}
