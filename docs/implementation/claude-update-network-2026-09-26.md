# Claude Code 2.1.283 升级失败排查

2026-09-26，用户报告 dogcodes 内从 Claude Code 2.1.282 升级再次出现“更新下载连接中断”。

## 结论

隔离对照复现了当前 HTTP 代理路径下的原生整包下载失败。同一更新器在禁用代理的隔离对照中成功升级。没有发现命令参数、目标版本、代理环境变量缺失、写入权限或制品校验问题。

这能定位到经过当前代理路径的下载问题，但不能单凭本次对照区分本地代理程序、上游节点、链路抖动，或 Claude 原生更新器与代理的交互问题。

## 证据

- 用户正式入口仍指向 `~/.local/share/claude/versions/2.1.282`。失败留下的 `versions/2.1.283` 为零字节，修改时间 03:55（America/New_York）。
- 运行中的 dogcodes 继承 `http_proxy` / `https_proxy`，值为 `http://127.0.0.1:7890`。Claude 设置文件没有覆盖代理变量。[官方说明](https://code.claude.com/docs/en/network-config)确认小写变量受支持。
- 官方 latest 返回 2.1.283；[官方 manifest](https://storage.googleapis.com/claude-code-dist-86c565f3-f756-42ad-8dfa-d59b1c096819/claude-code-releases/2.1.283/manifest.json)声明 darwin-arm64 制品大小为 225,036,032 字节。
- 对同一制品请求前 8 MiB：代理与直连均返回 206 并完整读取。因此地址可达或版本检查成功不等于整包下载可靠。
- 在两个独立临时 HOME 内复制同一份 2.1.282 原生更新器，执行 `claude update`，没有操作正式安装或发送模型请求。

| 对照 | 结果 |
| --- | --- |
| 当前代理 `127.0.0.1:7890` | 退出 1：`The connection dropped while downloading the update (attempt 3/3: aborted)` |
| 清除代理变量并设置 `NO_PROXY=*` | 退出 0：`Successfully updated from 2.1.282 to version 2.1.283` |

直连组的安装入口报告 `2.1.283 (Claude Code)`，文件大小和 SHA-256 均匹配官方 manifest，`codesign --verify --strict` 通过。

SHA-256：`d8cb1e5c79684cc12a8bfc813e3a2073406921b6245744b3009be3ab5651d21e`。

## 为什么再次出现

上次处理修复了错误提示、升级失败后的重试按钮和并发升级交互，正式安装靠人工断点续传恢复。应用仍直接调用 `claude update`，没有更改原生下载机制或代理路径，所以此次仍可发生同类失败。

本次是原因排查；没有更改全局代理设置、产品升级策略或正式 Claude 安装。应用当次失败没有持久化原始更新器输出，以上精确错误来自本次隔离复现，不冒充当次原始日志。临时安装副本在校验后清理，小型诊断日志保存在 `/tmp/agent-claude-283-evidence`。
