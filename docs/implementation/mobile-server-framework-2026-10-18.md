# 手机与服务端框架实施

2026-10-18：按用户要求开始实现；服务端位于 `../dogcodes-server`，手机端位于 `../dogcodes-app`，两个独立工程。RustDesk 的服务端及 Flutter 源码只读参考，未复制其文件、资产或协议。桌面正在进行的 UI 修改不在本次范围。

## 本轮交付

服务端：Rust / Tokio / Axum，公开协议/本机管理双监听器；健康/能力查询，短期双端票据、哈希存储和常量时间验证，一次连接占用、过期、撤销、断线结束、有界队列/帧/容量/流量、心跳与退出。管理路由不注册到公共监听器。框架只绑定 loopback，不连接桌面执行接口。

手机端：Flutter 官方生成的 iOS/Android 工程；电脑、会话、设置导航；相机扫码和贴上链接、邀请来源/版本/长度/时效校验；HTTPS 服务能力查询和友好错误；独立草稿存储原语；64 位字符串游标、operationId 请求模型、等待端点认证实现的 HostChannel 接口。草稿尚未接编辑器，扫描邀请不能创建可信设备；服务能力明确说明配对/业务/远控未接入。

两端：同一 service-info 固定样例及契约测试；真实 Rust 服务 + Dart WebSocket 客户端双向字节 smoke，临时随机端口/凭证，只使用合成数据并清理自身服务。各自 README、协议/接续文档、CI 和忽略规则齐备，签名材料不入库。

## 验证

- Rust：6 项 HTTP/WebSocket 整合测试通过，`cargo fmt --check`、`cargo clippy --locked --all-targets -- -D warnings` 通过。
- Flutter 3.47.7 / Dart 3.13.5：分析通过，9 项 Dart/Widget 测试通过；包括 QR 来源/时效、u64 精度、写操作身份、草稿隔离/重开、服务错误/重定向、导航/窄屏。
- 跨工程：`tools/smoke.py` 检查固定样例一致、双向字节转发及清理，exit 0。开发中发现并修复 Dart 提前取消 WebSocket 订阅导致 Ping/关闭资源残留；现先停止 Ping、等待对端 Close，再释放订阅，整条脚本正常退出。
- 本机 HTTP proxy 初次阻碍 Flutter tester 回环连接，设置命令级 NO_PROXY 后通过，未修改用户全局代理配置。
- SDK 安装在独立的 `~/.local/share/dogcodes-toolchains/flutter`，未更改 shell PATH。Android SDK 未安装；Xcode 没有可用 iOS simulator runtime，CocoaPods 未安装。原生两平台编译/安装与相机真机验证均未完成；CI 原生 debug 构建配置尚未执行。

## 明确未完成

没有安全配对/身份持久化、端到端加密、PC 网关或真实手机业务闭环；二进制中继不是加密证明。没有 ICE/TURN/直连、公网部署/地区实测、设备推送、画面采集/编码/解码/键鼠、网页预览、附件分块、离线会话缓存。未缩减原来确认的功能范围。

下一工作切片：先定跨语言成熟加密握手和固定向量，接 PC 配对记录/撤销与安全存储；再增加 PC 出站网关，把 PairedDevice 身份贯通共享服务；打通扫码→真实项目/会话→发送→流式→审批→撤销。复用现有 `submission_intents` 原请求核实，禁止把 daemon token 作为手机凭证或把手机伪装为 LocalUser。远控媒体可随后独立接入，不阻塞业务链路。


## 同日继续：端点安全与配对状态

使用者正在手动安装 iOS runtime 和 Android Studio，按要求继续不依赖模拟器的工作。在 `dogcodes-app/native/endpoint` 新增独立的 Rust rlib/cdylib/staticlib，通过 Snow 0.10 实作首次 XXpsk3、重连 XX、公钥固定、顺序加密与错误终止；Dart FFI 调用同一库，不自行实现密码算法。端点模块不加载到官方中继。

新增 host 配对 journal：邀请到期/取消/重新生成、同邀请码的并发消费、回执丢失核实、持久重开、撤销与 revision 租约、重新授权不复活旧租约/回执；写入失败或残留 pending 文件时停止授权，不自动清空。手机新增系统安全存储适配、单次身份初始化、损坏不轮换、匹配加密通道回执后保存 PC pin。Android 禁用应用备份，iOS 采用 this-device keychain 选项；系统插件本身尚未真机验证。

验证：Rust 8 项安全/状态/持久化测试、1 项覆盖两个模式的独立 Cacophony 固定向量测试通过；Dart 3 项原生 FFI/身份存储/配对回执测试，连同前一轮 9 项合计 12 项通过（设置原生库路径，未跳过）；严格 Clippy 和 Flutter analyze 通过。跨仓 smoke 新增 encrypted 模式，通过真实本机中继完成握手、双向加密数据、重放拒绝与正常退出。服务未获得端点私钥。

这一步仍不是手机到真实 PC 的可用配对：原生库尚未包进 iOS/Android，PC daemon 未接配对/网关，手机配对 UI 仍只读取邀请；官方服务能力继续返回 pairing/business=false。下一步固定端点库交付方式、原生打包与系统存储验收，再接 PC 的 PairedDevice 身份与真实会话闭环。先前“未完成”清单中端到端加密/配对状态指产品接入未完成；底层端点模块已有上述实现与证据。


## 同日继续：原生打包、PC 网关与首条业务闭环

以下进展更新前文的历史状态；不是全部远控需求完成。

- 加密模块的规范源码移到 `crates/remote-endpoint`；PC 直接依赖安全 Rust API，不启用 FFI。App 保留同版本源码快照及逐文件 SHA-256，`tool/sync_endpoint.py --check` 验证一致；两仓可以独立构建，不使用本机绝对路径依赖。官方服务不链接端点库。
- App 构建钩子自动生成 Android ARM64 `.so`、iOS device/simulator 静态库；产物忽略。iOS/Android 模拟器都已通过原生 Noise 握手/加密往返和真实系统安全存储读写测试。Android 当前仅 ARM64；其他 ABI 尚待适配。
- 桌面偏好菜单新增「连接手机」：短期二维码、复制邀请链接、设备列表/撤销，关闭窗口使该窗口邀请失效。调试接入本机服务；正式版本只接受 HTTPS。密钥及主机路由令牌进入系统 keyring，公开配置和配对 journal 留在后台服务目录；损坏/不可读不会悄悄轮换身份。
- daemon 自行维持出站在线通道与配对/重连连接，不依赖 React 窗口。官方服务新增无账号主机登记、在线通知、手机路由凭证与重连房间分配；持久化凭证摘要，不持端点私钥或会话正文。配对 journal 先持久化再发送加密 ACK；公钥固定与 Noise XXpsk3/XX 验证双方身份。
- 远程业务走白名单：bootstrap、Agent/模型/权限目录、submission_intents、项目订阅、时间线、待审批及审批回执。仅受认证连接生成 `PairedDevice`，忽略手机自报 actor；不能远程调用 HostOnly/internal、传 daemon token 或直接 Put 原始提交意图。
- 手机接入真实项目/会话、新会话发送、就绪且启用的 Agent、模型/思考强度、事件驱动时间线与权限审批。写前保存完整原请求/operationId，超时显示待核实；显式核实/重试使用原身份，放弃前先关闭原提交身份。审批保存原决定、版本和请求，失效时先建立旧决定栅栏。撤销禁止既有连接继续发起操作和交付结果；此前受理的任务仍由 PC 执行。
- 业务 JSON 使用 4-byte 长度前缀/Noise 分块、8 MiB 上限、有界发送队列和并发限制；邀请入场时效与已连接通道期限分离，避免五分钟二维码到期时中断正常会话。u64 协议元数据转为十进制字符串，不递归改写用户 payload。订阅补齐全局游标，过滤项目事件并在拥塞时补页。

### 本轮证据

- 服务端 8 项 HTTP/WebSocket 整合测试、严格 Clippy 通过。
- 手机 12 项普通 Dart/Widget 测试、3 项 macOS 原生 FFI 测试通过，Flutter analyze 无问题。
- Android 模拟器及 iOS iPhone 18 Pro 模拟器的原生加密/系统安全存储 integration test 各通过 1 项；不代表相机真机、Windows/Linux keyring 或所有网络环境已验收。
- PC 网关 4 项边界测试、Core 6 项审批相关测试通过；桌面配对测试覆盖 React StrictMode 下不会误取消当前邀请。桌面 TypeScript 类型检查通过。
- `dogcodes-server/tools/gateway_smoke.py` 通过：真实隔离 SqliteStore/Core/提交服务、合成 Agent 驱动，手机 Dart 使用生产加密/请求实现完成配对→发送→流式→审批→重连→撤销；证据检查只执行一次任务。使用临时目录/端口/身份，不读取用户数据库、密钥或真实模型账号。

复现：先用 `cargo build -p desktop --features remote-acceptance --bin remote-acceptance --locked` 构建专用测试入口，再按 server README 执行 gateway_smoke。该二进制与正常 desktop 分离，要求临时目录标记；不会因普通构建覆盖入口而误启动主程序。

### 2026-10-18 手机业务对接补充

- 网关同一连接可并存多个项目订阅（按项目替换，上限 32），手机可跨项目聚合会话；配对/重连回执附带电脑显示名称（不参与身份或授权）。
- 白名单新增只读 `quota_snapshots_for`，忽略手机传入的强制刷新。
- 配对邀请的首个握手等待改为直到邀请到期（15–300 秒），修复用户扫码超过 15 秒即配对失败（中继 410）。
- 验收夹具的合成驱动在中断时结束当前轮次，与真实驱动一致；`gateway_smoke` 新增手机停止、额度、重复订阅与电脑名称检查并通过。手机 iOS 模拟器真实 UI 端到端测试见 `../dogcodes-app/integration_test/pc_control_test.dart`。

### 使用与后续

本机服务运行方式、两模拟器配对步骤见 `../dogcodes-app/README.md` 和 `../dogcodes-server/README.md`（从对应仓库根目录读取）。PC 使用更新后的开发版本，偏好菜单打开「连接手机」。旧后台 daemon 若仍在执行任务，应等待空闲后正常退出/重开以载入新增网关，不强停任务。

尚待：官方公网 TLS 部署/准入与地区实测、真实 Agent 与真机联调、首次配对 ACK 丢失后的原邀请在线回执查询（目前需重新生成二维码）、多端竞争专项、草稿接入/离线缓存、完整会话管理、Diff/Git、文件附件、终端/运行/网页预览、推送、四平台画面/键鼠/剪贴板/多屏。服务能力 pairing=true 表示路由与端点配对已接入；business=false 指云服务不提供 PC 业务端点，并非手机不能调用 PC Core。remoteDesktop=false。

## 2026-10-19：workspace 网关扩展

文件、Diff/Git、终端、项目运行及对应失效订阅已在 PC 加密网关开放，复用现有后台服务与回执；不需要额外开放端口。方法白名单、参数与手机待接入边界见[远程 workspace 契约](remote-workspace-gateway-2026-10-19.md)。手机现有四个占位入口仍需调用这些接口，不能将本次网关实现视为手机页面已完成。
