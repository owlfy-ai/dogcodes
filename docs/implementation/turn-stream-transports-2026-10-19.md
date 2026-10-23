# PC TURN TCP/TLS 接入

PC 的 `remote-control` 工作区保留原有 P2P、Noise 与 WSS 协议，并为 TURN 增加 TCP 和 TLS over TCP。手机无需更改配对或业务协议。

| PC → TURN 服务 | 地址示例 |
|---|---|
| UDP | `turn:turn.example.invalid:3478?transport=udp` |
| TCP | `turn:turn.example.invalid:3478?transport=tcp` |
| TLS over TCP | `turns:turn.example.invalid:5349?transport=tcp`，可配置独立 443 入口 |

这里的 TCP/TLS 是 PC 到 TURN 服务的传输；TURN 分配的 peer relay 仍是 UDP。既不是 ICE-TCP，也不是 WebSocket。无需用户手工开放 PC 入站端口，公网部署仍需 TURN 服务及其 relay 端口可达。

## 实现

当前 registry `webrtc 0.21.1` 的 async adapter 会跳过非 UDP / secure TURN 地址。采用该版本库源的受控本地补丁（`vendor/webrtc`），保留上游许可证、原始源码校验和及改动说明；`rtc` 加密、ICE、DTLS、SCTP、TURN 协议状态机继续由版本锁定的上游库提供，没有复制 RustDesk 协议。

- 新 `turn-stream` 功能使用 Tokio TCP/Rustls；域名解析及连接有截止时间，多地址尝试并发，多个服务也并发协商。
- TLS 校验证书链、有效期及 DNS/IP 名称，发送对应 SNI；使用系统和公共根证书，不提供跳过验证的开关。私有 CA 可通过系统信任或标准 `SSL_CERT_FILE`/`SSL_CERT_DIR` 配置。
- 按 RFC 8656 处理 STUN 长度及 ChannelData padding；支持拆包/粘包，不添加 ICE-TCP 的 RFC 4571 两字节前缀。
- 认证、分配、Refresh、权限、channel binding 和撤销分配复用同一 TURN client。收发队列和写等待有界；连接关闭会取消读写任务，正常退出先发送认证 Refresh(0)，再以截止时间排空写队列。
- 瞬时队列溢出/写失败视为该连接失败，交由既有连接恢复及原 operationId 核实机制处理，不自动重放用户写操作。
- 修复 driver 空 socket future 的忙循环；UDP socket 不可用时不立即终止仍存活的 TURN stream。
- ICE 收集最多等待 8 秒；多网卡/不可达地址未完成时，保留已收集的有效候选生成 answer。没有候选才拒绝协商，让原加密中继继续工作。

PC 业务 RPC、SDP/Noise context、双端公钥 pin、权限审批及副作用回执保持原契约。TLS 服务器身份校验不能代替 Noise 的 PC/手机身份认证。

## 验证

安装本地 coturn 后执行：

```sh
python3 scripts/turn_acceptance.py
```

脚本仅监听 loopback，使用临时目录、随机凭据和私有测试 CA。测试 CA 仅传给测试子进程，不安装到 OS 信任库。双方强制 `iceTransportPolicy:relay`，断言 SDP 没有 host 候选及实际路径为 `turn-relay`，从而不能以局域网直连冒充中继通过。

覆盖真实 UDP/TCP/TLS、故意拆分字节流的 TCP 代理、固定公钥的新 Noise 握手、约 2 MiB 连续加密数据和反向回执、证书名称错误/不受信任证书、不可达地址。CI 的 Linux desktop job 会安装 coturn 并运行相同脚本。普通测试仍明确忽略需要外部 TURN fixture 的用例，不把 ignore 当作验收成功。

本机结果（2026-10-19）：7 项强制中继/失败场景通过；桌面远程网关 18 项测试通过；P2P 传输及原有 WSS 完整 Core 链路通过；iOS、Android 模拟器对更新后 PC fixture 的 P2P/流式任务/回退通过；Clippy、格式、发布输入检查和 Windows GNU 交叉编译检查通过。CI 配置已接入，尚未在 GitHub 执行。

三平台代码使用同一传输实现；Windows 交叉编译及 macOS 本机验证不能替代 Windows/Linux 实机运行、官方 443/TLS 入口、IPv6、Wi-Fi/移动网络切换及中国大陆/海外跨 NAT 验收。TURN/DTLS over UDP 和 TURN-over-WebSocket 不在本次范围。
