# Antigravity 重新检测失效排查（2026-09-29）

## 现场事实

- 本机 `~/.local/bin/agy` 为 1.2.13。原生日志确认 06:45 会话使用该版本，桌面数据库保存了成功回复；不能将本次故障归因于版本升级导致协议不兼容。
- 故障现场桌面后台服务持有一个从 06:45 开始的 stream-json 进程，聊天已完成，进程仍等待后续输入。
- 后续多次 `agent.probe_completed` 只有 `installed`，未回到 `ready`。CLI 独立 `models` 调用成功。
- 原生日志另有 Google API/OAuth 的间歇 EOF，随后 token 刷新和模型查询恢复；这不能解释桌面探针尚未启动就等待进程锁的情况。

## 原因

1. `Core::select_installation` 在选择相同路径时也删除 driver 和 quota provider。重新检测走这条路径，原有 Antigravity 会话失去驱动管理，但后台读取任务仍持有会话及 stdin，原生进程继续存活。
2. Antigravity 会话在进程整个生命周期持有全局凭证保护锁；这是已有的共享认证写入保护。
3. 扫描先记录 `installed`，再调用 `probe_ready`；后者等待同一把锁。旧进程尚未退出，扫描最终触发外层 90 秒超时，状态停在 `installed`。

## 修复

- 选择相同安装路径不再注销现有驱动及额度提供者；路径变更或移除仍会失效。
- Antigravity 重新检测仍执行可执行文件发现、版本及身份检查。只有指纹一致、原状态 Ready、同一安装的驱动仍在当前 Core 注册时，保留 Ready 及原驱动，不再启动竞争锁的流式探针。
- 二进制变化、首次检测、服务重启后无注册驱动时仍走原有协议验证。未改认证信息、凭证锁或审批 Hook。

## 验证

- 桌面回归 `antigravity_rescan_preserves_registered_driver_and_ready_status` 通过：覆盖相同路径重选与重复检测、二进制变化、冷启动持久化 Ready 不足以跳过验证，以及切换路径注销驱动。
- 扫描协调器回归、Core 的 48 项单元测试通过；Antigravity Driver 的 14 项离线测试通过，4 项真实环境测试未执行。
- 严格 Clippy 被已有 `project_commands.rs`（duration_suboptimal_units、collapsible_if）及 `running_permissions.rs`（wildcard_imports）告警阻止，未修改这些无关代码。

- 开发实例自动重载后，通过真实后台 `scan_agent` 连续重检两次，分别耗时 1.746 秒和 1.348 秒，均返回 Antigravity 1.2.13 / Ready。本轮没有发送新的模型聊天请求。


## 后续：欢迎页显示“授权待确认”

用户重检后的截图显示连接已识别但没有额度，原先只验证 Ready 的验收没有覆盖此状态。

- 原生 Driver 的只读额度测试复现 `agy quota timed out`（20.00 秒）。直接运行原生 `/usage` 则在 21.81 秒成功返回 `SUCCESS` 和四个有效额度窗口，说明 20 秒预算不足。
- 日志另捕捉到 Google 头像服务 `lh3.googleusercontent.com` 的 TLS handshake timeout，CLI 将它嵌套包装为 `Eligibility check failed`。旧分类会误触发授权退避，而非普通网络错误。
- 修复额度预算：Antigravity 原生查询 45 秒、Core 外层 50 秒、欢迎页与工作台查询 55 秒。其他 Provider 的原有预算不变。
- 网络错误不再触发授权退避；额度超时与网络失败分别返回安全诊断 `quota_timeout` / `quota_network_error`，页面明确显示错误原因，不伪造已登录。
- 原生只读额度测试修复后通过（10.89 秒）；没有发送模型提示词。前端相关 32 项测试、Antigravity 14 项离线测试通过；前端类型检查通过。
- 一次全量前端测试得到 497 通过、4 失败，未将整个前端测试集报告为通过；定向的欢迎页与额度刷新测试均通过。

- 最终开发实例后台 `quota_snapshots_for` 返回四个有效窗口、`confidence=provider_reported`、`diagnostic=null`；Gemini 五小时剩余 100%，每周剩余约 97.1%。该请求等待正在进行的原生读取后在 7.06 秒返回。最终 Core 48 项及 Antigravity 14 项离线测试、前端相关 32 项测试和类型检查通过。未通过原生 UI 自动化复核卡片文本（开发二进制未出现在可控应用列表）。
