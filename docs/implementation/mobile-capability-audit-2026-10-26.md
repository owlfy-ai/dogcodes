# 手机首版能力核对

依据已确认的 RA-04/RA-05 范围核对当前 PC、手机和服务端源码；本表不把所有桌面管理功能推导成新增手机页面。Agent 安装/升级/登录仍通过远控，文件仍只读，项目添加仍选择 PC 已有目录。

| 已确认范围 | 当前实现证据 | 尚未证明或明确缺口 |
| --- | --- | --- |
| 配对、多 PC、在线状态、撤销 | 手机 `mobile_runtime.dart`、`pairing_page.dart`、`security/identity_store.dart`；PC 绑定网关及服务端 PG/Redis 路由 | 系统安全存储、真实终止重绑、移动网络跨网仍待实机 |
| 新会话、Agent/模型/思考强度、发送/取消 | `new_session_page.dart`、`agent_picker.dart`、`mobile_operations.dart`；原生模型目录来自 PC | 安装/登录/升级不新增手机原生管理页；实际供应商验证不能由合成 Driver 替代 |
| 流式内容、长历史、置顶/归档、搜索、Fork | 时间线尾页/分页、增量合并及 `sessions` 下已有页面；`session_features_widget_test.dart`、`stream_reconnect_widget_test.dart` 等 | 真实大历史与弱网后台恢复待设备验收 |
| 审批及回答 | `approval_card.dart`、`question_form.dart` 和固定命令回执；PC 复核原 Run/请求 | 无法表达的原生问题格式明确提示到 PC，不猜测回答格式 |
| PC 已有目录添加项目、项目文件/附件 | `add_project_page.dart`、`files_page.dart`、`composer_attachments.dart`、`task_attachments.dart` | 文件写入仍交 Agent/远控；无须新增目录创建/克隆或项目编辑器来扩大范围 |
| 审查、修复及日常 Git | 本轮审查/取消/主 Agent 修复；多 Git 根的 Diff、暂存、提交/推送、拉取/同步、AI 说明已接入 | 已补齐历史提交分页、合并父提交及分支只读比较；真实 Git 仓库/设备端到端验收仍待 |
| 多目录终端、运行与 App 网页预览 | 终端 directoryIndex/cwd 与原 ID 重连；运行按 directoryIndex/commandId；`preview_page.dart`/`preview_proxy.dart` 转发 HTTP/WS | 真实 PTY、WebView/HMR、系统后台切换与安装包待实机，不把接口存在等同端到端完成 |
| 草稿、附件、本机缓存与清除 | `composer_state.dart`、`local_workspace.dart`、绑定隔离及已有普通测试 | 系统备份/安全存储/应用被终止的设备验证仍待 |
| Agent 安装状态、额度与保持唤醒 | 安装/额度只读显示；PC 原生任务/远控活动及版本化电源偏好 | 不扩展为手机安装/登录/升级；真实电源和锁屏行为待系统验证 |
| 远控、逐屏、键鼠/文字、手动文本/图片剪贴板 | `remote_desktop_page.dart`、原授权租约、媒体与输入管线；源代码及既有本地回归 | Windows/macOS/X11/Wayland 与两类真机联调、真实弱网、HDR/光标/GPU 实际表现未整体验收 |
| 后台通知与静音 | 服务端 outbox/provider、手机 EMAS deviceId 登记、静音/四类开关、点击核实绑定并打开会话、Android 厂商弹窗和 iOS APNs 参数已接入；见手机 docs/push-notifications.md | 凭据与供应商真实收件、后台/强退点击、地域网络待实机；不把 SDK 初始化或构建成功等同推送已验收 |

历史比较差异已补齐：PC `CompareMode`、远程 allowlist 及 `History/Branches/Query/File` 沿用既有实现；手机 `pc_api.dart` 增加 rootId/reference/parent 和只读列表调用，`git_reference_picker.dart` 提供 50 项提交分页/分支选择，`changes_page.dart` 接入父提交选择。历史页面隐藏选择/暂存/提交并禁止 Git 写操作；切换 Git 目录返回工作区并保留各目录草稿。重连关闭旧选择器、重读当前比较，旧响应不覆盖新内容。没有新增 PC 协议、依赖、分支切换或历史管理功能。

通知渠道统一 EMAS。官方 aliyun_push 1.2.0 已接入，按两端私有构建参数配置；空配置不初始化。服务端按 emas_ios/emas_android 发送，iOS 指定 APNs 环境，Android 使用固定任务通道及辅助弹窗。配置与签名/厂商前提见手机 docs/push-notifications.md；凭据与实机收件按原约定随后验证。

手机历史比较新增 6 项页面测试已通过；PC 比较语义另核对既有源码。随后通知偏好新增 4 项 API/回环 HTTP 测试和 6 项页面测试，覆盖绑定/原凭据、2048 项响应、离线 PC、会话/电脑静音、版本冲突、丢失保存回执和解除绑定后的迟到结果。通知设置使用现有版本检查；失败只读取真实值，未添加持久操作队列或自动重复 PUT。后台推送未启用提示保留。

最新普通手机回归 `/tmp/dogcodes-emas-all-tests.log`：431 项通过、9 项原生按需测试未启用；手机全项目 analyze 无问题，格式与差异检查通过。Android/iOS 两端构建通过；服务端参数专项及隔离 PG/Redis 集成通过。未运行真实 Git 写入、模型、推送、模拟器或设备；不以本地 fixture/回环 HTTP 代替云端及实机验收。

通知 SDK/deviceId 登记与点击定位的代码/本地验证已接通；凭据与实机验证随后安排。其余范围按 `CURRENT_WORK.md` 原八包继续，不依据假设增加新的工作包。

## 2026-10-27：网页预览的 WebSocket 顺序与关闭

PC `preview_ws_write` 在原 socket 的 writer 锁内要求连续 sequence；手机原先的 async listen 回调不会串行等待请求。真实手机回环 WebSocket 加私有 HostLink 延迟首条回复时，旧代码确实同时发送两条 RPC，回归在最大并发 2 而非 1 处失败，日志 `/tmp/dogcodes-preview-proxy-before.log`。跨网调用的调度/传输不保证这些并发请求保持接收顺序，PC 正确拒绝序号冲突也不能代替手机有序发送。

手机沿用同一接口，以 asyncMap 暂停下一条事件，等待本条原序号确认；失败或不匹配时关闭原流，不重发本条或继续发排队内容。读方向仍独立长轮询。关闭只结算本次 socket 一次，捕获本地流错误、握手失败和已关闭响应；页面关闭后不再开始下一读写，迟到的远程 open 也会按原 socketId 关闭。代理 stop 幂等，bind 晚到时关闭自己的本地端口；页面离开或 WebView 创建失败时，尚未交给页面的预览/代理由原打开流程释放。

新增三项普通回环测试验证慢回复下有序发送、stop 后迟到连接/无新 reader/幂等，以及未知写入后停止排队且只关闭原 socket 一次。修复后专项三项和完整 Flutter 444 项通过，9 项既有原生按需用例未启用；全项目 analyze 无问题，修改文件格式及差异检查通过。日志 `/tmp/dogcodes-preview-proxy-after.log`、`/tmp/dogcodes-preview-suite.log`、`/tmp/dogcodes-preview-analyze.log`。

没有新增协议/依赖/业务重试、数据迁移或修改 PC/服务端生产代码；同一 HostLink 调用层供 P2P/中继使用，但本轮只运行本机真实 HTTP/WebSocket 和私有 RPC 端点，不证明实际 WebView、HMR 框架、PC 端到端或跨网换路已验收。iOS/Android 原生 WebView 创建及其失败路径未跑设备测试。原八包范围保持，Mac 原生创建问题和实机/部署门槛不以本轮通过关闭。

## 2026-10-27：同一预览服务的重定向

私有真实 HTTP 服务返回 `302 Location: http://127.0.0.1:<PC端口>/signin?...` 时，原接口原样转给手机；手机实际代理在另一端口，导致地址指向手机上的错误服务，localhost 也不符合原 WebView 的主机限制。新增 Core/SQLite + Preview Host 集成在旧路径复现，日志 `/tmp/dogcodes-preview-redirect-before.log`。

PC 仍不跟随重定向：只将 HTTP、localhost/127.0.0.1、同一选定端口且没有用户凭据的 Location 转为根路径，保留原编码、query 与 fragment。普通相对路径原样保留；其它地址/端口不改投到当前服务，根路径以双斜杠开始的地址也不转成会被浏览器当作新主机的相对引用。手机导航按本页实际代理的 scheme/host/port 一起核对，防止把另一个手机本地端口当成本页预览；初始加载与导航复用同一个 origin。没有扩展访问目标或新增 UI 选项。

新增 PC 集成用例用七种实际 302 响应验证本服务两种主机写法、协议相对 URL、原相对路径、其它目标/端口及双斜杠路径；始终返回原 302，不在 PC 重发/跟随。手机增加实际 HttpClient → 回环代理 → 私有 RPC 的重定向用例，验证浏览器侧相对跳转继续落在同一代理，编码 query 保持、fragment 不变成 HTTP 路径。手机现有三项 WebSocket 顺序/迟到关闭回归继续通过。

macOS/Linux desktop 各 350 项通过（8/11 项按需未启用），手机完整 445 项通过、9 项原生按需未启用，Flutter analyze 无问题。最终两端 desktop all-targets 严格检查及 Windows GNU 同范围静态严格检查通过；首轮仅因测试夹具的固定列表使用 Vec 未过严格检查，已改数组，不添加 lint 豁免，产品逻辑未随此调整。日志 `/tmp/dogcodes-preview-redirect-{after,suite-macos,linux,phone,suite-phone,analyze}.log`、`/tmp/dogcodes-preview-redirect-strict-final-{macos,linux,windows}.log`。Windows 未实际执行。

另核对本机已解析 Vite 7.3.6 的 client.mjs/config.js：默认 HMR 使用模块所在页面的 host/port，已有 WebSocket 隧道能够承接该地址形式，未依据猜测重写项目 JavaScript。这是源码核对及协议层本地测试，不是实际 WebView/Vite 热更新或自定义跨端口 HMR 的设备验收。本轮没有连接真实项目/设备、调用模型、切换日常 Core 或提交推送，原八包和实际网络/设备范围保留。
