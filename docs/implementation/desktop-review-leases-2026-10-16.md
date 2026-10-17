# Diff 阅读者租约

日期：2026-10-16。对应 DT-08/09 的共享 Diff 前置改造。独立阅读者租约已实现；后续同日已完成 ReviewHost 服务所有权迁移，见[服务记录](desktop-review-service-2026-10-16.md)。完整可靠性与实机验收仍未完成，不能据此放行 DT-08/09。

## 问题与实现

前端会合并同时发起的相同查询，两个 ReviewPanel 可以拿到同一个 snapshot ID。旧实现只按快照保存一个到期时间，任一面板释放都会删除另一面板仍需要的保护；独立请求重排时，释放后的迟到续租还可能恢复该保护。

- 每次面板挂载/更换快照创建独立 UUID leaseId，续租和释放带单调递增 revision。仍保留查询合并，避免重复扫描同一工作区。
- 后端按 snapshotId + leaseId 维护阅读者状态。只有该阅读者被释放；任一有效阅读者仍可阻止快照淘汰。较旧请求忽略，同版本同操作幂等，同版本不同操作拒绝；已释放身份不能重新续租，新视图必须使用新身份。
- 新快照保留 120 秒领取宽限，首个合法阅读者请求结束宽限；之后每个阅读者独立使用 120 秒 TTL，前端每 30 秒续租。所有阅读者释放后，快照可被淘汰；尚未成功领取租约的迟到视图若遇淘汰，需要刷新。
- 重复确认不延长 TTL。释放/过期记录保留到快照淘汰，用于阻止迟到请求恢复旧状态。每快照最多 256 个阅读者身份，配合最多 16 份快照限制内存；超过后明确要求刷新，不静默删除仍有排序作用的记录。
- 卸载时停止定时器，释放只发送一次。已释放视图或旧 revision 的失败不会覆盖当前视图错误；当前续租失败仍提示比较状态需要刷新。
- 继续校验窗口可访问的任务，以及快照和任务归属。租约是临时缓存保护，不是跨设备授权，也不持久化恢复过期快照。
- 原生验收脚本同步使用新 request 载荷及独立阅读者身份。

## 验证

集中验证结果：

- `cargo test -p desktop --lib snapshot -- --nocapture`：5 项通过，exit 0。覆盖独立阅读者、过期、乱序释放/续租、同版本冲突、重复确认、身份上限、快照淘汰和任务/根目录/文件匹配。日志 `/tmp/dogcodes-review-leases-native.log`。
- `pnpm --filter @dogcodes/desktop exec vitest run src/reviewLease.test.ts src/reviewClient.test.ts src/ReviewPanel.test.tsx`：3 文件、78 项通过，exit 0。包括两个真实面板共享快照时仅释放自身、客户端顺序字段、迟到错误隔离及既有 ReviewPanel 行为。日志 `/tmp/dogcodes-review-leases-focused.log`。
- 桌面类型检查、验收 JS 语法检查及 diff 空白检查通过。类型日志 `/tmp/dogcodes-review-leases-types.log`。

第一次前端命令使用 `test -- ...`，分隔符导致文件参数未用作筛选，意外启动全套。已明确识别并停止本次 Vitest，收取 exit 130，随后使用上面的 exec 命令只执行指定文件。原日志 `/tmp/dogcodes-review-leases-frontend.log` 不算全套通过证据。没有打包或实机 UI 验收。

## 服务迁移依赖（已由后续工作块接通）

源码依赖复核确认，不能只移动查询缓存后保留另一份 GUI 写入状态：

1. `review.rs`：查询、冻结正文读取、续租/释放、SnapshotOwner 磁盘引用、淘汰；暂存、丢弃、Git action 和提交说明生成共用 ReviewHost gate。
2. `review_file_actions.rs` 与 `review/batch_file_actions.rs`：预览、当前路径校验、忽略、批量丢弃/忽略依赖同一快照与门禁。打开本机文件管理器可留本端适配，但路径授权在共享服务核实。
3. `review/project_analysis.rs`：项目审查材料从同一快照按任务归属生成；不能因 GUI 退出丢失唯一来源。
4. `turn_undo.rs`：逐轮撤销/状态读取共用 ReviewStore 路径和 gate，完成后的 UI 通知也需核对多客户端同步。
5. `lib.rs::core_execute`：永久删除的快照清理仍持有 GUI gate 并清除本地 ReviewHost，迁移需与后台删除协调，避免删除后残留服务缓存。

服务迁移后用两个真实本机 RPC 客户端验证共享查询/读取、独立释放、断连与服务重启、项目范围变化和相关写操作；外部副作用另补持久意图/回执及结果未知核实。本记录当时仅验证阅读者身份与乱序保护；以上服务迁移现已由[后续工作块](desktop-review-service-2026-10-16.md)接通并验证，完整副作用可靠性及三平台实机仍未完成。
