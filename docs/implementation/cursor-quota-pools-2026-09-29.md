# Cursor 分项额度

Cursor 的订阅页分别显示 Cursor Models（Cursor Grok、Composer）和 Other Models（第三方模型）。原生 CLI 2026.09.23-86fc751 的 `/usage` 已经返回分项，但仍使用 Auto / API 旧名称。此前适配器只读取 Included，因此丢失了分项。

本机 Cursor 桌面代码 `auto-spillover-ui` 明确将 autoPercentUsed 映射为 Cursor Models，将 apiPercentUsed 映射为 Other Models；CLI 的 usage-data 和 usage-modal 使用同一对字段。

官方说明：[Models and pricing](https://cursor.com/docs/models-and-pricing)。Cursor Models 超额可能消耗 Other Models 或按量付费，因此任一分项归零不表示整个 Agent 不可用。

## 实现

- 继续通过原生 CLI `/usage` 只读查询，不读取或导出凭据，不发送模型请求。
- v2 解析器分别返回 `cursor-models` / `cursor-other-models`，保留分组 scope；支持 Auto / API、带冒号的紧凑布局及显式模型池名称。
- 只有旧客户端未报告分组时才兼容 Included 总值，不用总值推算分项。分组不完整或越界时拒绝快照。
- 欢迎页、额度详情分别显示两个每月额度池；紧凑主圆环代表 Cursor Models，并明确标注名称，不计算跨组总额。
- 账期重置保留原生月日文本，不虚构年份或精确时间，不参与额度封锁判断。

## 验证

2026-09-29 通过运行中的本地服务强制刷新，约 7 秒返回，无 diagnostic：Cursor Models 已用 20%，Other Models 已用 100%，Pro，重置 Oct 4，按量付费关闭。与用户截图一致。

解析回归覆盖旧版总值、双池、紧凑布局、显式名称、缺失分项及超过 100% 的无效数据；前端回归覆盖双池卡片、月度重置、圆环名称和分项用尽不封锁整个 Agent。
