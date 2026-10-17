# 后台服务接口与手机功能映射

日期：2026-10-17。目的：说明后续手机 App 的每项首版业务功能（范围见[远控评估](remote-access-assessment-2026-10-16.md) §1.1、§1.3）由当前后台服务的哪些方法承担，哪些已经用“非窗口客户端”测试证明可用，哪些要在远程阶段（RA-02/RA-03）补齐。方法名取自 `apps/desktop/src-tauri/src/daemon.rs` 及其分派模块的实际代码。

## 1. 传输与身份（现状）

- 本机 loopback TCP，长度前缀 JSON 帧，每个请求一条连接，`service.json` 中的随机令牌和协议版本必须匹配。服务端是方法白名单，未知方法返回错误。
- 每个服务方法都在 `daemon/access.rs` 中归类为读取、写入、订阅或仅限本机（服务生命周期、工作区锁、Agent 检测、本机打开、本机路径产物）；服务拒绝未归类的方法，测试扫描所有客户端调用点，新增方法必须先归类。远程网关只暴露非“仅限本机”的方法，并把设备权限映射到这些类别。
- 所有调用目前都以本机用户（`ActorRef::LocalUser`）身份执行。手机不能直接连接此端口：远程阶段需新增网关，负责配对设备认证、把设备身份映射为可信请求上下文，并按能力裁剪方法；不能让手机冒充本机用户（RA-02/RA-03，见远控评估 §3.2 第 2 条）。
- 已接受的命令不随客户端断开而取消；变更命令都带操作身份，可在回复丢失后用同一身份查询或重试。

## 2. 功能映射

“已用协议客户端测试”指测试只通过服务协议调用（`CoreClient::Remote`，不经过 Tauri 窗口）完成该功能，见第 3 节。

| 手机功能 | 服务方法 | 状态 |
| --- | --- | --- |
| 会话列表、项目、Agent 安装状态 | `bootstrap`、`history_preferences`、`agent_preferences` | 已用协议客户端测试 |
| 添加 PC 上已有目录为项目 | `register_project`（服务内规范化、复用、互斥）、`update_project` | 已用协议客户端测试；宿主目录浏览接口待 RA 阶段新增 |
| 新建会话并发送、切换 Agent 后发送 | `submission_intents`：`start` | 已用协议客户端测试 |
| 断线后核实、继续或放弃原发送（含另一端发起的） | `submission_intents`：`list`/`get`/`resolve`/`continue`/`retire` | 已用协议客户端测试（手机发起、桌面发现并完成） |
| 流式消息与状态 | `subscribe_project`（单项目 + 全局设置/安装/额度流）、`timeline`、`timeline_snapshot`、`sync_events` | 已用协议客户端测试项目隔离；按设备授权过滤待网关 |
| 模型、思考强度、权限选择 | `model_catalog_for_task`、`permission_catalog`、`registered_executors` | 方法已在服务中 |
| 审批 | `pending_approvals`、`execute`（`RespondToApproval`）、`prepare_approval_response`、`approval_receipt`、`invalidate_approval_response` | 方法已在服务中；Core 目前要求本机用户身份，待 RA-02 贯通设备身份 |
| 停止当前回复 | `execute`（`CancelTurn`）、`command_receipt` | 方法已在服务中 |
| 审查、开始修复、复审 | `review_executors`、`start_turn_review`、`turn_reviews`、`cancel_turn_review`、`review_source`、`review_repair` | `review_repair` 服务驱动已测；其余方法已在服务中 |
| Diff、暂存、提交、推送 | `review`（`query`/`file`/`lease`/`index`/`discard`/`git_action`/`git_operation`/`git_advance`/`git_close`/`commit_summary`/`batch_files`/`operations` 等）、`review_watch` | 服务持有并有专项测试 |
| 项目文件浏览 | `workspace_roots`、`workspace_directory`、`workspace_preview`、`workspace_find_files`、`workspace_watch` | 服务持有；路径限定在项目范围 |
| 任务附件上传 | `attachment_save`、`attachment_receipt`、`attachment_read` | 服务持有；大文件分块待远程传输设计 |
| 终端 | `terminal_start`/`terminal_list`/`terminal_get`/`terminal_read`（offset 长轮询 + 缺口）/`terminal_write`/`terminal_resize`/`terminal_stop` | 已用协议客户端测试（手机启动、桌面读取、尺寸共享）；屏幕快照待手机阶段 |
| 项目运行 | `project_runs`、`project_runs_analyze`、`project_run_edit`、`project_run_operation`、`project_run_open`、`project_runs_watch` | 服务持有并有专项测试；网页预览转发待 RA 阶段 |
| Fork | `fork_task`、`fork_receipt` | 方法已在服务中 |
| 额度 | `quota_snapshots_for` | 方法已在服务中 |

桌面专属、不提供给手机的入口：系统文件选择框、在本机应用中打开/显示文件、导出到本机路径、诊断导出、窗口管理、Agent 登录与升级（按约定通过远控在 PC 界面完成）。

## 3. 协议客户端测试

`apps/desktop/src-tauri/src/daemon/tests/remote_client_tests.rs` 在真实 loopback 服务上用两个协议客户端（“手机”和“桌面”）运行：

1. 手机注册项目并新建会话发送，桌面立即看到项目和会话。
2. 桌面禁用 Agent，手机读到同一设置；手机此时发送被拒但原意图被服务保存；桌面列出该待处理发送，恢复 Agent 后由桌面继续完成，手机读到相同的结束结果。
3. 手机启动终端，桌面列出并读取输出；手机调整尺寸后桌面读到新尺寸，桌面输入后手机读到结果。
4. 项目订阅：两个项目各新建会话，订阅第一个项目的客户端收到该项目新会话的事件和全局设置变更，从不收到另一项目的会话或项目流事件；不存在的项目拒绝订阅。

## 4. 远程阶段仍需补齐

- 网关、配对设备身份与按能力授权（含把 Core 中要求本机用户的操作改为校验设备权限）。
- 单连接多路复用：当前每个订阅和请求各占一条连接，远程传输需复用连接并设置背压。
- 终端屏幕快照、附件分块与续传、网页预览转发、宿主目录浏览。
- 推送通知与离线缓存（手机端）。
