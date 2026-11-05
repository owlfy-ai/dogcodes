# 本机 Cursor 启动与停止延迟

## 定位

本机现有 `turn-startup-timing.jsonl` 的两次 Cursor 请求中，
`command_validation_append` 分别耗时 17416.7 ms、17350.9 ms。
提交时指定了模型，Core 会再次调用工作区模型目录查询；ACP 模型目录查询
独占一个临时进程和 `session/new`，且等待该进程完整退出后才返回。
它重复了模型选择器已经取得的原生目录。

本机 Cursor 的元数据探测（不发送 prompt）实测：进程启动约 732 ms，
initialize 约 379 ms，首个 session/new 约 8771 ms；同一进程的后两个
session/new 约 2768 ms、3085 ms。冷启动有 CLI 自身的开销。

旧进程关闭路径还有 5 秒自然退出等待；取消无回应则先等待 10 秒。

## 修改

- 仅本机 Cursor 的提交前校验复用服务已有的模型目录。正式会话仍通过
  原生模型配置接口应用并确认选择，失败时不会发送 prompt。
- 本机 Cursor 的物理进程退出宽限期从 5 秒缩短到 250 ms。结果仍等待
  原进程、子进程和后台任务确认退出；不把计时器到期当作完成。
- 只有当前会话独占连接时，本机 Cursor 的无回应取消在 1 秒后走已有的
  进程终止兜底。存在其他会话时保留原来的等待与连接保护。
- 其他 Agent 和远程 Cursor 保持原行为。

## 验证

ACP 回归、Core 模型目录缓存回归和 store 的 preparing_cancellation 集成测试，
覆盖卡住的子进程、模型拒绝时不发 prompt、元数据结果等待真实退出，
以及取消超过一秒但仍有共享会话时不终止连接。

真实 CLI 无模型调用测量命令：

```sh
DOGCODES_ACP_BINARY="$HOME/.local/bin/cursor-agent" \
  cargo test -p acp-driver --test local_cursor_latency -- --ignored --nocapture
```

本机结果：readiness 8921.07 ms；已初始化进程的 session/new 2810.27 ms；
确认物理关闭 589.06 ms。这不是首个模型回复或完整应用链路的耗时，
没有声称消除 Cursor 原生冷启动及模型服务等待。
