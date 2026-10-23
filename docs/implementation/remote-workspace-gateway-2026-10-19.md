# 文件、变更、终端与运行的远程网关

2026-10-19，`remote-control` 分支。PC 已接入下列后台服务；手机现有四个入口仍是占位交互，需要按此契约实现页面。后续扩展已加入原生远控基础接口、开发服务转发、分块附件和直连协商，完成状态见[后端核对](mobile-ui-backend-completion-2026-10-19.md)。

## 连接与权限

沿用扫码绑定、Noise 加密、官方服务中继及连接复用，不新增 PC 监听端口。仅通过当前未撤销的绑定设备连接访问。绑定授予个人设备完整功能，Agent 自身审批规则不变。远程调用不能指定执行 actor，也不能获得本地 daemon token；原有 HostOnly/internal 入口仍不开放。

每条请求仍为 `{version:1, hostId, requestId, method, params}`。`requestId` 必须非空且最多 128 字节，在一次连接中保持唯一。结果为 `{type:"response", requestId, result}`，请求错误为 `{type:"response", requestId, error:{message, code?}}`。`remote_capabilities`（`params:null`）返回 workspace 方法、订阅方法及资源限制，旧 PC 不支持此方法时手机应提示升级 PC。

## 请求方法

表内参数就是 `params`，不是 Tauri invoke 参数。`taskId`、`projectId` 来自 bootstrap，`root` / `directoryIndex` 为从 0 开始的项目目录索引；文件路径相对于选中的目录。文件查询必须有有效会话上下文，当前没有“仅凭 projectId 浏览文件”的接口。

| 方法 | 参数 | 结果 / 语义 |
| --- | --- | --- |
| `workspace_roots` | `taskId` 字符串 | 主目录及附加目录数组 |
| `workspace_directory` | `{taskId,root,path}` | `{name,path,directory}` 数组；根目录 `path:""` |
| `workspace_preview` | `{taskId,root,path}` | `{text,image}`；图片为 data URL，二进制可能都为 null |
| `workspace_find_files` | `[taskId,query,hideNoise]` | `{files:[{root,path}],partial}` |
| `review` | `{operation,arguments}` | 原后台 Diff/Git 请求契约，见下文 |
| `terminal_start` | `{operationId,projectId,directoryIndex,taskId?,commandId?,label,cwd,command?,cols,rows}` | TerminalInfo；`cwd` 必须在当前项目对应目录内；`command:null` 启动交互 shell |
| `terminal_list` | `projectId` 字符串 | TerminalInfo 数组 |
| `terminal_get` | 终端 `id` 字符串 | TerminalInfo，使用原 id 重新附着 |
| `terminal_read` | `[id,after]` | 长轮询 `{info,from,next,gap,bytes}`；`bytes` 是原始字节数组，可包含 ANSI，按终端流解码 |
| `terminal_write` | `[id,bytes]` | null；单次输入最多 64 KiB |
| `terminal_resize` | `[id,cols,rows]` | null；所有客户端共享同一个终端尺寸 |
| `terminal_stop` | `id` 字符串 | TerminalInfo；显式结束进程，断开页面或连接不等于停止 |
| `project_runs` | `taskId` 字符串 | 当前平台 RunCatalog；首次缺少有效配置时沿用桌面后台分析行为 |
| `project_runs_analyze` | `{operationId,taskId,expectedGeneration,executor?,overwriteGeneration?}` | `{Ok:RunCatalog}` 或 `{Err:{kind,code?,message}}` |
| `project_run_edit` | `{operationId,taskId,generation,directoryIndex,commandId,edit}` | 同上；edit 为 `{action:"rename",label}` / `{action:"delete"}` / `{action:"pin",pinned}` |
| `project_run_operation` | `[taskId,operationId]` | 原分析/编辑 RunOperation 或 null |
| `project_run_start` | `{operationId,taskId,generation,directoryIndex,commandId,cols,rows}` | TerminalInfo；从冻结的目录、配置代次与命令 id 启动，通过终端接口查看输出 |

`project_run_start` 在内部复用 `project_run_open` 的后台服务，不开放原本的 HostOnly RPC 名称。不允许通过 `terminal_start.projectLaunch` 伪造运行项绑定。文件预览服务拒绝绝对路径、`..` 和逃出项目目录的符号链接。

终端 `outputEnd`、读取结果 `from`/`next` 以十进制字符串传输；`after` 接受字符串或 JSON u64。按 `next` 继续读取；`gap:true` 表示较早输出已超出后台保留窗口，手机需明确显示截断。进程状态、退出码和 epoch 以 TerminalInfo 为准。手机必须根据状态停止无意义的轮询，不应把长轮询超时解释为进程已停止。

## Diff 与 Git

`review` 直接复用 `apps/desktop/src-tauri/src/review/service.rs` 的 `ReviewCall`（snake_case operation，camelCase arguments）。开放：

`query`、`operations`、`lease`、`file`、`history`、`branches`、`index`、`discard`、`git_action`、`git_close`、`git_advance`、`git_operation`、`commit_summary`、`batch_files`、`file_operation`、`file_close`、`preview`、`ignore`、`undo_status`、`undo`、`workspaces`。

不开放本机查看路径的 `reveal_path` / `workspace_path`；已追加开放 AI 审查启动 `start_project_review`，复用原服务。普通项目文件界面只用只读 workspace 方法；Diff 里的暂存、丢弃、Git 与撤销使用原有受约束的操作接口。

示例按顺序执行，所有 id 都替换为真实结果；Git 操作 id 使用 UUID，保留同一请求的原始参数：

```json
{"method":"review","params":{"operation":"query","arguments":{"taskId":"TASK","mode":"uncommitted"}}}
{"method":"review","params":{"operation":"file","arguments":{"taskId":"TASK","snapshotId":"SNAPSHOT","rootId":"ROOT","path":"src/main.rs"}}}
{"method":"review","params":{"operation":"git_action","arguments":{"operationId":"UUID","taskId":"TASK","rootId":"ROOT","action":"commit","message":"Update implementation","stageAll":true,"snapshotId":"SNAPSHOT"}}}
{"method":"review","params":{"operation":"git_operation","arguments":{"taskId":"TASK","operationId":"UUID"}}}
```

快照不是文件路径授权凭证：后台仍校验会话、当前项目目录及快照归属。手机使用自己的 `lease` 阅读租约保护快照，不能沿用桌面窗口的阅读者 id；查询结果过期时刷新并让用户核实，不能自动改参数重发提交。Commit & Push 沿用 GitWorkflow 的 `nextOperationId` / `parentOperationId` 与 `git_advance`，不得让手机在未知结果时自行新建一次 push。

## 状态订阅

| 方法 | 参数 | 重新查询 |
| --- | --- | --- |
| `workspace_watch` | `{taskId,root,path}` | 对应目录、预览或搜索结果 |
| `review_watch` | `taskId` 字符串 | 快照、Git/文件操作回执 |
| `project_runs_watch` | `taskId` 字符串 | 运行目录、操作回执及终端列表 |

成功先返回 `{subscribed:true,subscriptionId:requestId}`，然后发送：

```json
{"type":"notification","subscriptionId":"watch-1","method":"review_watch","event":{"kind":"invalidated"}}
```

`event.kind` 也可能为 `heartbeat` 或 `error`（携带 code/message，error 后该订阅结束）。这是失效通知，不是完整业务数据；重连后先重新查询事实，再重新订阅。不同于会话事件的 `type:"event"`，不能混入时间线解析。

取消：发送 `method:"unsubscribe", params:"watch-1"`；返回 `{unsubscribed:true}`，不存在则 false。连接关闭或设备撤销也释放全部订阅。取消确认前已经排队的旧通知可能仍到达，手机需忽略已取消的 subscriptionId。旧 `subscribe_project` 仍按项目替换订阅，成功结果额外包含 subscriptionId，也可通过同一取消入口释放；它的历史回放与实时事件格式不变。

## 可靠性与限制

- 每连接最多 32 个订阅、16 个并发普通请求；超限返回对应请求错误。
- 消息主动按 16000 字节明文加密分片（WSS 接收仍兼容旧 32 KiB），JSON 消息上限 8 MiB；结果超限返回 `error.code:"response_too_large"`，保留连接。大图片预览和很大 Diff 需要手机缩小范围；分段附件使用独立 `attachment_upload_*` / `attachment_read_chunk` 接口，见下文。
- 终端启动、运行启动、Git/文件修改、运行分析/编辑沿用后台的固定操作身份和持久回执。断线后先查原回执/终端，禁止用新 operationId 自动重做同一副作用。
- `terminal_write` 是原始实时输入，没有幂等重放协议；断线后禁止自动重发。模型驱动的分析和提交摘要同样不能因客户端等待超时而盲目重启。
- UI 订阅失效不影响已受理的后台任务。远端设备共享 PC 上真实的项目、Git 索引、运行配置及终端状态；手机应展示版本冲突与未知结果。

## 验证范围

原生网关专项包含真实临时 Git 仓库、PTY、持久运行目录夹具、文件越界和符号链接拒绝、操作身份冲突、订阅上限/取消/断连清理及超大结果错误。测试不调用真实模型、不推送仓库、不读取用户文件。端到端手机四个页面仍待接入；Windows/Linux 的真实终端与文件监听验收待对应实机进行。


## 增量接口

`remote_capabilities` 返回 workspace/business/desktop/power/preview/transport 方法列表及限制。所有接口均在已认证、未撤销的配对连接内；不能将下面方法改为公开 HTTP 操作。

- `attachment_upload_begin/status/commit/abort`：完整 `AttachmentUploadRequest` 对象 `{uploadId,name,mimeType,byteLength,sha256}`；chunk 为 `[request,offset,base64]`，offset 接受十进制字符串，分块 256 KiB。status 返回 uploading/offset/maxChunkBytes 或 completed/attachment。单附件 25 MiB，提交验证摘要后才发布，同身份不同内容拒绝。`attachment_read_chunk` 为 `[managedPath,offset,limit]`，返回 bytes(base64)/next/eof。
- `workspace_attachment_info`：`{source:{taskId,root,path},uploadId}` 返回完整上传身份；import 使用同参数额外带 `request:原身份`，文件改变时拒绝。成功后即使源文件删除，原身份仍可取得已保存附件回执。
- `host_directories`：`{path:null|绝对目录,offset:0}`，返回 entries/next，选择已有 PC 目录；`register_project` 参数 `[ProjectRegistration]` 复用本地注册契约。
- 会话/历史、Fork、上下文、审查、修复和 artifact 沿用原 daemon 参数；`execute` actor 总是被网关覆盖为认证设备。`task_artifacts/read_artifact` 同样不能指定别人的 actor。Quota 不强制刷新供应商。
- `power_preferences` 参数 null；`power_preferences_set` 为 `{operationId:UUID,expectedRevision:十进制字符串,enabled:bool}`。默认 true，只抑制任务/远控期间的 idle sleep；返回 enabled/revision/active/error。
- `device_status` 参数 null；`remote_desktop_displays` 返回 id/name/坐标/尺寸/scale/primary。open 参数 `{monitorId,control:true|false}`，返回 leaseId、15 秒时效和 inputSequence；frame `{leaseId,maxWidth?,quality?}` 返回 JPEG base64/尺寸，最高 5fps；select `{leaseId,monitorId}`；input `{leaseId,sequence:"1",event}`，事件为 move(x/y 0..65535)、button、key、text、scroll；close `{leaseId}`。
- 原生 lease 仅属于当前连接，断线/撤销/过期释放键鼠；换到直连须结束原 lease 并重新打开，不能跨连接搬用 leaseId 或重放输入。OS 锁屏或锁屏状态未知时屏幕/键鼠/剪贴板拒绝，Agent 仍可工作。
- `clipboard_get`：`{kind:"text"|"image"}`；set 为 `{kind:"text",text}` 或 `{kind:"image",bytes:PNG-base64}`。文本 1 MiB、编码后 PNG 约 4 MiB 限制，均为手动操作。
- `preview_open`：`{taskId,port}` 返回 previewId；HTTP `{previewId,path,method?,headers?:对象,body?:base64}` 返回 status/headers(键值对数组)/body(base64)，请求/响应 4 MiB；不跟随重定向、不使用系统代理、禁止自动重发。close 参数 previewId 字符串。
- `preview_ws_open`：`{previewId,path,protocols?:[]}` 返回 socketId/protocol/sequence；read 参数 socketId 字符串，返回 text/binary/pending/closed；write `{socketId,sequence:"1",text}` 或 bytes(base64)，单消息 1 MiB，重复序号拒绝；close 参数 socketId。read 最多等待 5 秒，调用方继续轮询。每连接最多 4 个预览/8 个 WS，5 分钟闲置失效，断线释放，不暴露 PC 监听端口。

手机 WebView 需要自己的受控本地代理，将页面 HTTP 和 WS/HMR 映射到上述请求；按 host/preview 隔离 cookie/站点存储，重写本地 URL/Location，不能将 routeToken/daemon token 转发给项目。该手机桥本轮未实现。大响应返回明确错误，不等于无限制通用 HTTP 隧道。

直连、TURN、恢复路由、通知与部署的完整契约在独立 server 的 `docs/transport-and-mobile-backend.md`；手机当前仍 WSS，尚未接入自动 P2P。跨平台原生捕获/输入及正式媒体管线仍需验收，不能仅因方法出现而显示所有系统已可远控。

## 2026-10-22 输入协议更新

远程 `terminal_write` 已由 `terminal_input` 替换，能力标记为 terminalInputStreams；支持 inspect/open/write、epoch/revision/stream/序号、身份绑定、部分输入失效与只读核实。每批原始输入最多 16 KiB，不持久重放键盘内容。详见 [终端输入通道](terminal-input-streams-2026-10-22.md)。历史表格的原始写接口仅描述早期实现，本地旧接口仍保留。
