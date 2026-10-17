# M1-S5：Redaction ruleset v2

更新日期：2026-09-03

## 实施状态

已在既有 Context 初建、编辑重哈希与 TestReport 持久化路径升级默认 Redactor 至 v2，落实
详细设计 §19.3 的高熵候选与凭据格式覆盖增量。没有启用 Raw Frame Recorder，也没有将
文本正则误称为 §7.5 的结构化字段 allowlist。后续结构化策略与离线诊断首切片已完成，见
[`m1-s5-raw-diagnostics.md`](./m1-s5-raw-diagnostics.md)；生产 Recorder 尚未接入。

## 新增规则与顺序

1. 已知 JSON 双引号敏感字段的字符串值：api key、access/refresh/id token、client secret、
   private key、password、authorization、cookie 等；处理转义引号，保留匹配字段的 JSON 引号。
2. 既有敏感 assignment 扩展 client/id 字段并保留分隔符、引号；PEM private-key 块整体屏蔽，
   截断且没有 END 的块屏蔽到输入结尾。
3. 既有 Provider token 特征；Bearer 与 Basic Authorization header（含短值）；URL 的 userinfo
   整段移除并保留 scheme/host/path。Basic 需要 header 上下文，避免改写普通 “basic integration”。
4. 高熵候选；之后仍执行邮箱和用户主目录规则。

全部是本地确定性操作，不探测凭据有效性、不连接 Provider、不使用模型，也不保留 secret
尾部。占位符记录类别且不可复用，计数只记录真正发生的替换；再次处理同一安全文本计数为 0。

## 高熵候选定义

- 扫描至少 32 字节的连续 `[A-Za-z0-9_+/=-]` 候选，须含字母+数字或大小写混合。
- 使用整数碰撞熵阈值：令 `S = sum(count(byte)^2)`、`N = byte_length`，纯十六进制候选
  满足 `8*S <= N*N`，其余候选满足 `16*S <= N*N` 时屏蔽。分别对应 Rényi H2 至少 3/4 bit。
  实现只用整数计数，不用浮点 log，从而避免边界候选造成跨平台 hash 差异。
- 不按疑似 hash 的外观豁免任意正文：正文中的 digest/长标识也可能误报；短值、低熵密码、
  其他字母表/拆分编码可能漏报。只有既有强类型 content hash/Artifact refs 不走自由文本扫描。
- 这不是 Shannon 熵或第三方探测器的复刻，也不是凭据安全保证。启发式需要接受误报/漏报，
  参见 [detect-secrets 的设计说明](https://github.com/Yelp/detect-secrets/blob/master/docs/design.md)。
  不能因此开放未知 Raw Frame 字段或取消用户预览。

## Context 引用与版本语义

- 实测发现 `task/<TaskId>/events?...` 整段可能被通用启发式误报；Core 仅对完整匹配的
  `canonical_history` pointer 豁免 locator 文本扫描：TaskId 可解析、query 与 provenance
  after/through 一致、pointer ID 一致、content hash 为 64 位 hex。title/trigger hints 仍脱敏。
- 任意 Provider/User locator、凭据 URL 或不满足该结构的 locator 继续脱敏；不是按路径前缀
  或 substring 全局放行。此判断仅保护文本引用，不授予跨 Task 查询权限。
- 默认 `redactionRulesetVersion = 2`；新的 package/hash 反映规则变化。升级前未确认的 v1
  preview 需重新构建并审阅，不能将旧 hash 当作新规则下的确认。
- 已持久化 Event/Artifact/CAS 不重写，没有数据库迁移或自动“升级”旧内容；查看旧 Artifact
  仍显示其原始规则版本。Raw Diff ruleset 0/原始字节不变。
- Canonical 用户输入不擅自修改；Core 集成测试证明原始 Task 标题保留，而生成的 Context
  不含测试凭据。再次重哈希已脱敏 package 时文本、累计计数与 payload hash 均稳定。

## 验证与入口

- `crates/redaction/src/lib.rs`：9 项测试，覆盖 JSON 转义、已知 token、Basic/
  URL/完整或截断 PEM、高熵/低熵、重复处理、Unicode 与约 1 MiB 单候选整体屏蔽。
- `crates/core/src/context.rs`：编辑后的高熵值重新脱敏、重复 hash 稳定、冻结
  pointer/hash 保真、任意 URL 不豁免；Core preview 集成测试证明生成与 Canonical 来源分离。
- Store TestReport 集成测试证明高熵值和 quoted cookie 在持久化前替换，metadata 为 v2，
  同一原始 bytes 通过受限 Artifact 检索校验且仍可重建 Projection。
- 当前基线：Rust 106 passed、1 ignored（含后续结构化诊断回归）；前端 25 passed；fmt/clippy/typecheck/build 通过。
- Tauri/Vite 启动通过，真实 app data 只读检查 `user_version = 2`、`quick_check = ok`；开发
  进程已停止。本轮未新增浏览器交互或原生保存 UI 验收，不扩大此前 UI 证据范围。
- 本轮只用合成凭据进行测试，没有读取真实凭据、运行模型 Turn 或更新外部 Agent。

## 下一 agent 接手

1. 结构化 schema/path policy 与离线导出/二次检查已完成；下一步审计旧 M0 summary report，
   扩展方法策略前补版本依据和合成失败用例。quoted-key 正则不能替代结构策略。
2. 接入生产 Recorder 分片/压缩/保留和仓库 fixture CI，沿用版本/来源/预算约束。不能把
   进程内用于审批响应的原始 request ID/payload 提前脱敏破坏协议语义；不能把离线副本
   当作原生协议回放或 runtime verified 证据。详见 Raw Diagnostics 实施记录。
3. 原生保存/取消 UI 验收仍按 Artifact 专项记录；Windows 与完整敏感数据审计仍待完成。
