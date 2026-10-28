# 审查启动取消与关闭确认

日期：2026-10-26。工作包 2 的有限修复，复用已有临时会话管理，不新增协议、迁移、持久化启动系统或通用重试机制。

此前，审查启动直接 select 取消/timeout，会丢弃仍在创建的原生请求；正文结束后又忽略了关闭结果。两处都可能在原资源尚未结算时过早显示结束。

本轮将审查接入已有 `auxiliary_session::Session`，保留原工作目录及附加目录，只持有路径，不删除项目目录。创建阶段的 1 分钟限制只结束前台观察；迟到会话仍由原 Driver/原 scope 清理，取消后不再发送 prompt。

原退出证明在 prompt 前捕获。正文结束后只关闭一次；关闭未知则保留活动审查和 Core 保护，并展示待确认提示。取消保持“正在取消”，获证后才结算为“已取消”；成功正文也等待原退休完成才显示“已完成”。不把原生查找表项消失当作退出证明。

执行流 panic 会先清理原会话，再记录失败；已收到的输出在等待清理期间保留。最终取消与完成仍沿用原锁排序，没有扩展终态存储重试策略。

验证使用私有内存 Driver、临时 SQLite 和临时项目。新增五项实际 Core/SQLite 回归：完成后关闭阻塞时仍能取消；关闭错误接续原证明且注册表替换不改目标；流 panic 保留部分输出；取消启动后的明确拒绝不伪造关闭；原生表项消失仍等待已捕获的证明。原启动取消测试改为释放原创建并验证迟到资源实际清理，取消前不再直接断言结束。

macOS/Linux Core 各 119 项、macOS desktop 348 项（8 项按需测试未启用）及全部存储 180 项通过；Linux 审查/准备取消/异常清理共 52 项集成通过。macOS 全工作区、Linux Core/存储/Driver API 三包及 Windows GNU 对应三包 all-targets 严格检查通过；Windows 是静态交叉检查，没有执行 Windows 二进制。格式与 diff 空白检查通过。日志：`/tmp/dogcodes-review-lifecycle-final-macos.log`、`/tmp/dogcodes-review-lifecycle-strict-macos.log`、`/tmp/dogcodes-review-lifecycle-final-linux.log`、`/tmp/dogcodes-review-lifecycle-final-windows.log`。

未运行安装的 Agent、真实模型、用户桌面/键鼠/剪贴板或手机设备，没有切换日常 Core、提交或推送；三个已有开发服务容器保持运行。

整体 Core 强退/重启、未知启动的持久化恢复仍在原计划内。本轮没有据假设增加新工作包；后续先核对 PC/手机日常功能的明确缺口，再处理有实际证据的恢复问题。
