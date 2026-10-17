# 附件发送链路实现与验收（2026-09-14）

## 已实现

- `SubmitTurn`、`PromptRequest` 新增独立的 `attachments` 元数据；提示词保持原文，允许只发送附件。
- 截图粘贴后由应用保存副本，无需用户先创建本地文件。支持图片内容的协议在发送时读取副本并编码图片字节。
- 普通文件传递可读取的引用，不把文件正文塞进提示词。
- 对话中渲染图片缩略图和文件卡片，隐藏内部缓存路径。附件元数据保存在 `turn.queued` 事件中，普通时间线和历史分页读取时恢复；重建投影后仍保留。
- 图片预览读取接口仅允许读取应用附件目录内的文件；发送前检查文件存在、数量和总大小。最多 16 个附件，总计 25 MB；图片格式为 PNG/JPEG/GIF/WebP。
- Base64 图片内容只进入原生协议，不写入事件历史。

## 各 Agent 的协议映射

| Agent | 图片 | 普通文件 |
| --- | --- | --- |
| Codex | `turn/start.input` 的 `image.url`，使用 data URL | 独立 text block 中的文件引用；Agent 可通过只读终端命令读取 |
| Claude Code | `user.message.content` 的 Base64 image block | 独立 text block 中的文件引用 |
| Kimi | ACP image block | ACP resource_link，文件 URI |
| Cursor | ACP image block | ACP resource_link，文件 URI |
| Grok Build | ACP image block | ACP resource_link，文件 URI |
| CodeBuddy | ACP image block；CLI 可能暂存为自己的图片文件并请求读取审批 | ACP resource_link，保留完整路径作为 name |
| ZCode | `session/send.attachments` 的 image/dataBase64 | file/localPath |
| Antigravity | 应用缓存图片，独立 text block 传引用，原生 `view_file` 读取 | 独立 text block 中的文件引用 |

## 实测发现并修复的问题

1. Grok 1.0.30 没有声明 `promptCapabilities.image`，但真实 ACP 图片输入可用。仅为已验证版本保留兼容规则；其余 ACP Agent 遵守初始化能力声明。Grok 握手也没有提供 Agent 版本，桌面注册时回退到安装检测已取得的版本。
2. ACP 的能力错误此前被统一改写为“连接已关闭”。现在保留能力不可用的具体原因。
3. CodeBuddy 2.150.0 的本地资源转换优先使用 `name`，再考虑解码 URI。仅传 basename 会把附件错误地解析到工作目录。现在该 provider 的 resource_link.name 使用完整路径；界面仍显示原文件名。图片按原生协议发送，CLI 自己缓存后的读取请求仍正常走审批。
4. Antigravity 复用活跃会话时原本只比较权限策略，没有比较目录。新增附件目录后现在重建原生进程并恢复会话，以让 `--add-dir` 生效；未修改用户全局配置或启用跳过权限。

## 真实验收

通过应用驱动发起调用，使用项目自带 Kimi 图标（文件名 sample.png）和包含随机校验码的临时文本文件。图片目录与工作目录分开，文本文件名包含空格和 `#`，检验路径引用及 URI 转义。只批准测试文件的只读操作；CodeBuddy 的原生缓存图片经字节比对后允许读取。Antigravity 审批桥正常启用。

| Agent | 图片内容辨认 | 文件校验码读取 |
| --- | --- | --- |
| Codex | 通过 | 通过（批准只读 cat） |
| Claude Code | 通过 | 通过 |
| Kimi 0.42.0 | 通过 | 通过 |
| Cursor 2026.09.10-fd3934a | 通过 | 通过 |
| Grok Build 1.0.30 | 通过 | 通过 |
| CodeBuddy 2.150.0 | 通过 | 通过 |
| ZCode | 通过 | 通过 |
| Antigravity 1.2.2 | 通过（view_file） | 通过 |

Cursor 返回中文“黑色”，初版验收程序只匹配英文 black，导致断言误报；回复中的图像特征和随机文件校验码均正确，验收程序已支持中英文。初版验收程序还漏读 ACP message 类型的展示事件，并在 Antigravity 测试入口漏分发审批 Hook；这些是验收工具问题，已修正后完成对应重测。早期测试拒绝了文件读取审批时，不将“完成回合”视为读取成功。

可复用验收入口：`crates/store/examples/attachment_acceptance.rs`。该入口需要本机已安装并登录对应 Agent，会使用真实额度，只读临时测试文件，不自动运行于常规测试。

```sh
cargo run -p agent-store --example attachment_acceptance -- grok
# 参数也可为 codex、claude-code、kimi、cursor、codebuddy、zcode、antigravity。
```

## 自动验证

- 前端：251 项测试通过，TypeScript 类型检查和生产构建通过。
- Rust：驱动、核心和存储相关单元测试通过；包括 Base64 编码、URI 转义、CodeBuddy 完整路径、版本兼容规则、空提示词附件发送、驱动收到附件、历史分页/投影恢复、Antigravity 附件目录变更。
- 桌面 Rust 构建通过。

这些结果覆盖当前安装版本和测试选用的模型；普通文件的解析能力仍取决于 Agent 自己的读取工具和当前权限，文件引用不等同于对任意二进制格式的理解。
