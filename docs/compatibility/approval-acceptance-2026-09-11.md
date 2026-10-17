# Coding Agent 真实审批验收（2026-09-11）

## 最新 ZCode 复核

用户要求重新真实调用后，当前代码再次通过拒绝、单次批准两个分支（42.90 秒，1 passed / 0 failed）。拒绝收到 1 次审批、无文件、正常完成；批准收到 1 次审批、工具成功、标记文件内容正确、正常完成。日志：`experiments/approval-acceptance/2026-09-11/zcode-approval-reconfirm.log`。下方“初次验收结果”是修复前历史，不代表当前 ZCode 状态。

修复为保存原生选项的 `response`，按用户选择的 `optionId` 查找并作为 JSON-RPC `result` 原样返回；不再只返回 `{optionId}`。真实测试走 Driver，不代表桌面或灵动岛 GUI 点击全链路验收。

## 方法与边界

经用户明确授权，调用项目实际 Driver 启动本机 Agent，在独立临时目录中请求执行固定命令：`printf DOGCODES_APPROVAL_OK > approval.txt`。ZCode 使用相同临时目标的绝对路径。测试分别选择原生拒绝与单次批准选项，并核验终态及真实文件内容。仅匹配预期命令的请求可以批准，其他请求拒绝。

Codex、Claude、Cursor、Grok、Kimi、ZCode 使用默认需审批策略（Driver 的默认参数路径）；Antigravity 显式选择 `desktop-approvals`。这些测试不修改用户对话中保存的策略。

这是实际 Agent、实际协议、实际审批响应和文件系统结果验收，不是模拟 Agent。审批按钮由测试程序通过 Driver 响应，并非在桌面界面手工点击；因此不代表完整 GUI 端到端验收。另运行了 ApprovalDetails 和 PermissionPicker 共 5 项前端测试，通过。

## 初次验收结果（修复前）

| Agent | 拒绝分支 | 批准分支 | 结论 |
|---|---|---|---|
| Codex | 收到审批、拒绝后无文件 | 批准后内容正确 | 通过 |
| Claude Code | 收到审批、拒绝后无文件 | 批准后内容正确 | 通过 |
| Cursor | 1 次审批、拒绝后无文件 | 1 次审批、批准后内容正确 | 通过 |
| Grok Build | 1 次审批、拒绝后无文件，结束为失败终态 | 1 次审批、批准后内容正确 | 执行门禁通过；拒绝的终态体验需注意 |
| Kimi | 1 次审批、拒绝后无文件 | 1 次审批、批准后内容正确 | 通过 |
| ZCode | 1 次审批、拒绝后无文件 | 1 次审批、已响应批准，但未生成文件 | 不通过，批准分支重复复现 |
| Antigravity | 未收到审批，测试文件直接生成；重复复现 | 同样未收到审批即执行，无法验证批准回传 | 严重失败：需审批策略未阻止未批准执行 |

### ZCode

两次批准分支均表现为 `approvals=1, allowed=1, completed=true, marker_written=false`。复测中 Agent 文本明确说明审批请求失败、命令未执行。不能以 TurnCompleted 判定工具成功。当前可确认审批请求到达 Driver，但批准后的原生执行闭环失败；具体协议兼容原因尚未确定。

### Antigravity

专用可执行探针使用与桌面相同的 `approval_bridge::run_hook_if_requested` 入口，避免普通 Rust 测试二进制不能处理 Hook 参数的问题。先运行 Hook 配置验证，再显式选择桌面审批策略。两次拒绝分支均未收到 ApprovalRequested，命令已执行、标记文件存在且复测内容正确。批准分支也为 `approvals=0, approved=0, completed=true, file=true`，标记内容正确，仍然没有实际响应审批的机会。当前 Hook 存在性检查不能证明执行拦截有效，不能将这一策略视作可靠审批门禁。测试没有自动修复或切换用户实际任务的权限。

## 复现入口

- `crates/acp-driver/tests/real_approval.rs`：Cursor、Grok、Kimi。
- `crates/codex-driver/tests/real_approval.rs`。
- `crates/claude-driver/src/lib.rs`：`real_approval_acceptance`。
- `crates/zcode-driver/src/lib.rs`：真实审批测试，新增返回文字诊断。
- `crates/antigravity-driver/examples/approval_probe.rs`：支持真实 Hook 回调的独立探针。

真实测试会消耗模型额度；仅在明确授权下运行。日志位于 `experiments/approval-acceptance/2026-09-11/`。

## 第一轮修复与复测（Antigravity 后续结论见下节）

- **ZCode 已修复并通过真实复测**：原生 `interaction/request_permission` 的响应要求使用所选选项的 `response` 对象（包含 `decision`，以及可选权限更新），之前错误地只回传 `optionId`。现保留并原样回传该对象。拒绝：1 次审批、无文件；批准：1 次审批、工具成功、文件内容正确。10 项单元测试通过。
- **Antigravity 已封堵绕过执行，桌面审批功能尚不可用**：原实现给 `desktop-approvals` 同样附加了 `--dangerously-skip-permissions`，但 Hook 虽显示启用，真实运行中没有触发。改变 Hook 工作目录仍无法拦截；删除绕过参数后，两次真实命令均未生成文件，但没有审批请求，均失败终止。现在只有显式完全访问才追加绕过参数，需审批模式在启动 CLI 前返回明确的能力不可用错误。此为故障隔离，不算审批通过/拒绝闭环修复；恢复该模式必须先有可靠原生传输并通过真实验收。
- Antigravity 增加“未验证传输不得启动 CLI”的回归测试；普通模型/流式测试显式选择完全访问，避免依赖已停用的审批传输。
- Grok 拒绝后的失败终态来自原生返回；拒绝已有效阻止写入，保留原生结果，未将失败伪装为执行成功。

修复日志：`zcode-approval-fixed.log`、`zcode-unit-fixed.log`、`anti-fixed-probe.log`、`anti-native-probe.log`、`anti-unit-fixed.log`。初次失败证据保留在上表及原始日志中。

## 参考 codex-host 后的最终修复：Antigravity 通过

参考 `~/Workspace/opensource/codex-host/docs/antigravity-tool-approval.md` 与 `packages/adapters/antigravity/src/question-bridge.ts`。该项目保持 Hook 临时目录直到 bridge dispose；我们的 Rust 异步任务只捕获 listener/token，未捕获 TempDir，导致 start 返回时目录被删除。检查 Hook 时目录尚在，因此配置检查通过，但执行阶段不再有 Hook。

现在异步服务显式持有 TempDir，结束时再清理，恢复需审批模式与原生 skip + Hook 组合。未通过配置验证仍不得启动。没有改变用户项目目录或全局 Hooks 配置。独立原生对照实验证明附加目录与 `--new-project` 均能拦截，因此保留原有项目隔离。

真实 Driver 复测（本机 agy 1.2.1）通过：

| 分支 | 审批请求 | 审批等待期间 | 终态与文件结果 |
|---|---|---|---|
| 拒绝 | 1 次，回传 deny | 等待 500ms 后仍无文件 | 完成，无目标文件 |
| 批准 | 1 次，回传 allow_once | 等待 500ms 后仍无文件 | 工具成功，目标内容精确匹配 |

测试使用临时目标的绝对路径，避免多个 workspace roots 下“当前目录”的歧义；审批白名单匹配实际 CommandLine 字段。先前相对路径测试虽然报告工具成功，但未在断言目录找到文件，不计为通过。最终退出码 0，日志 `anti-approval-final.log`。

12 项 Driver 单元测试通过，包含新增 Hook 目录存续与结束后清理回归，以及批准、拒绝、取消、错误令牌、外部会话的测试。仍属于真实协议/文件系统测试，不是桌面 GUI 手工点击验收。

## Grok Build 拒绝终态修复

原生实测确认：选择拒绝后 `session/prompt` 返回 `stopReason: Cancelled`，不是原生执行错误。之前 ACP Driver 映射为 TurnInterrupted，核心层又将它保存为 `turn.failed`，造成正常拒绝被显示为失败。

现在仅在 Grok、本轮记录了用户拒绝、且未主动停止的情况下，将 Cancelled/Refusal 映射为 TurnCompleted，说明为“已拒绝工具操作，本轮已结束”。每轮重置审批状态，终态清理未决请求；不改变其他 Agent、主动停止、连接错误、额度错误的结果。

真实批准与拒绝复测均通过（`grok-fix-real.log`）：拒绝 1 次审批，completed=true，文件不存在；批准 1 次审批，completed=true，文件内容正确。16 项单元测试通过（`grok-fix-unit.log`），覆盖拒绝映射以及其他终态不会被掩盖。以上修复取代早期“保留拒绝失败终态”的处理结论。
