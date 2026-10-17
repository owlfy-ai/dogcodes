# 悬浮岛移除与共享通信保留（2026-10-16）

按用户要求移除悬浮岛功能。用户正在开发手机 App，用于远程指挥电脑上的 Coding Agent，因此本次保留桌面与手机 App 可复用的状态和控制能力。

## 移除范围

- 删除前端 `Island`、`IslandLauncher`、窗口路由、ViewModel、专属样式及预览入口，移除主窗口的启用按钮。
- 删除 Rust 悬浮岛窗口实现、启动恢复、启用偏好读写、窗口专用事件及 `island_*` IPC 命令。
- 删除悬浮岛 capability、原生验收脚本和专属测试，清除共享审批/问答组件中的悬浮岛布局分支。
- 移除仅供悬浮岛使用的 AppKit 直接依赖及 macOS 透明窗口私有 API 配置。
- 重新生成桌面构建产物和 Tauri capability 清单；历史实施记录、截图与验收证据保留，并注明已退出当前范围。

## 共享能力保留

| 能力 | 保留入口 | 用途 |
| --- | --- | --- |
| 任务与运行状态 | `BootstrapSnapshot.companion` / `CompanionProjection` | 桌面会话列表的审查状态标记，以及手机 App 可复用的状态投影 |
| 读取状态与事件同步 | `CoreClient.bootstrap`、`sync_events`、`subscribe` | 获取状态、实时事件与断线后补齐 |
| 任务控制 | `CoreClient.execute` / `CommandEnvelope<CoreCommand>` | 创建任务、选择 Agent、提交消息、取消轮次及响应审批等 |
| 原生审批与问答 | `CoreClient.pending_approvals`、`RespondToApproval`、`PendingApproval` | 复用原生选项、版本校验、幂等处理与 Driver 回传 |
| 审查控制与历史 | Core / 后台服务已有查询、启动及取消接口 | 复用审查执行、结果和会话历史能力 |

删除的 `island_snapshot`、`island_approvals`、`island_respond` 是要求调用者为悬浮岛 Webview 的 Tauri 包装，内部委托上述共享接口；没有独立的手机网络服务。`island_focus_task` 只负责显示、聚焦桌面主窗口并发送窗口事件。

本次没有改变 Core/后台服务通信协议或删除共享 DTO，也没有新增手机网络传输、配对或对外监听。现有后台服务连接仍受原有本地通信边界约束。

## 验证

- `pnpm build`：类型检查和生产构建通过；源码与产物中不再包含悬浮岛代码或入口。
- 前端审批、问答、会话列表、启动和 Tooltip 相关测试：6 个文件、53 项通过。
- `cargo test -p agent-store --test conversation_companion`：2 项通过，覆盖共享审查状态投影及持久化会话重命名。
- `cargo check -p desktop --features review-acceptance`：通过，确认删除专属验收入口后该构建仍可编译。
- `cargo test -p desktop --lib`：111 项通过、7 项忽略、1 项 Antigravity 重扫测试超时失败；单独重跑该测试仍在约 10 秒后失败，日志为 `stage=process_spawn`。本次未修改该测试及对应驱动实现，也未将 Rust 测试套件记为全部通过。
- `git diff --check`：通过。
