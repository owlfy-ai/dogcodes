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
