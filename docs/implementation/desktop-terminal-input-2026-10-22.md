# 桌面终端输入代次与恢复（2026-10-22）

## 交付范围

主窗口与独立审查窗口的 TerminalPane（包括运行项目输出）使用同一 terminal_input inspect/open/write 协议，与手机共用 PC TerminalHost 的 CAS 版本、流身份、连续序号、摘要和原生排空规则。打开或重连输出不创建输入流；首次用户键盘、粘贴、IME 或 TUI 鼠标操作才请求输入。普通聚焦和输出触发的 DSR 回答不自行争夺输入。

原生产入口 `core_terminal_write` 和本地 RPC `terminal_write` 已移除。低层无序写入只保留为 Rust 测试夹具；CoreClient 的同名测试辅助方法实际走新序号协议，不恢复生产回退。手机公开接口不变。

## 信任边界与租约

- 桌面：JS 调用 `core_terminal_input {request}`；Tauri 从真实 WebviewWindow 取得 label，按原资源限制检查主窗口/审查任务的访问范围，不接受 JS 传入的 owner。CoreClient 使用本地认证 RPC `terminal_input [window, request]`；服务以 `local-window:<label>` 作为 owner。流 ID 在每个视图中独立生成，同一窗口的两个视图也不能用旧流写入。
- 手机：仍由远程网关在认证后从 PairedDevice actor 推导 owner，调用相同 TerminalHost.input_operation。不能把桌面的 window envelope 当作远程输入请求。
- PC 检查 terminalId、后台 epoch、CAS revision、streamId、连续 sequence 和 SHA-256。接管会让旧流失效，并等旧原生写入排空；重复同一请求只读回执，不再次写入；旧 owner、旧流或旧版本的迟到请求被拒绝。
- 本地 ping 声明 `terminalInputWindowProtocol:1`。桌面在发送本地新协议前检查；旧 Core 忙碌时仍可用于输出/审批，但不退回原始输入，也不强制重启它。此项不改变远程 terminal_input 的消息格式或数据库格式。

## 前端顺序与未知结果

terminalInput 在一次视图绑定中维护单路写入、最多 8 KiB 一批及 64 KiB 待发窗口；它不是累计会话输入上限。超量会明确暂停并丢弃待发队列，避免继续发送被截断的命令。每批发送前冻结原 epoch/流/版本/序号/摘要/字节数；回复丢失时只 inspect，不重发原字节。成功证明必须匹配上述身份，且是 owned/applied、无 uncertain、written 等于原字节数。

applied 只证明原字节完整写入 PTY，不证明 shell 命令已执行成功；命令结果仍通过终端输出和进程状态查看。

结果未知时暂停输入、禁用 stdin、丢弃未发送内容，保留原证明信息。输出继续显示；后续 running 状态不能把输入错误清掉或重新启用键盘。核实按钮只读；继续输入在提示用户先检查终端内容后，以新 CAS 流明确承接，不重发原内容或启动新终端。核实/继续成功后，仅在当前面板仍可见时返回终端焦点。

输入控制器与输出 xterm 实例分离。输出自动重连、同一资源的 API 包装更新均保留原未知身份；重连不能偷偷开启新输入。离开页面、暂停或替换准备过程后，迟到的 get/inspect/hash 不能继续 open/write。已发出的原请求可能已到达 PC，仍按未知结果处理，不假称撤销。

不把按键、粘贴正文或逐次输入写入持久恢复日志。后台重启后 epoch 不匹配就拒绝旧输入，不用新进程执行旧命令。其它进程/会话的持久操作回执和原生 terminal_stop 恢复继续使用各自协议。

## 本地证据及边界

- PC Rust 完整库：331 通过，7 项原有按需用例未运行；macOS/Linux 严格 Clippy 通过。
- 桌面前端完整：998 通过；最终受影响输入/窗口/键盘/Tooltip 用例 53 通过；类型检查和生产构建通过。
- Linux 隔离容器：44 项 terminal_ 相关用例通过/3 项按需跳过；真实 Noise/WSS/WebRTC 工作区网关用例通过。
- 新服务联调用独立 PTY、临时项目/任务及实际本地 RPC：主窗口写 A，重复相同写入不重复；审查窗口不能写主窗口的流，接管后写 B；手机接管后旧两方迟到请求拒绝，最终读取仅 INPUT:ABC。测试窗口 label 为认证本地客户端的模拟，未启动真实 Webview；手机调用认证远程入口，原有加密网关测试另行验证传输。
- 旧服务夹具只返回旧 ping，断言没有第二条修改连接；前端替身覆盖丢失回复、只读核实、跨视图/手机接管、输出重连/API 切换仍保留未知身份、准备期间离开、超量输入、错误完整写入证明、被动 DSR 与 TUI 鼠标。

本轮未安装或运行手机模拟器、切换日常 Core，未操作用户真实终端/屏幕/键鼠。服务端及手机代码未变。Windows、真实 Webview/系统输入与跨设备实机验收按既定安排后置。

下一步继续原生驱动强退/资源收尾审计；远控原生硬件加速、特殊来源、更高编码能力以及全部实机/TURN 续期/弱网、通知凭据与服务部署等原有范围仍保留，不能据本节宣布整个目标完成。
