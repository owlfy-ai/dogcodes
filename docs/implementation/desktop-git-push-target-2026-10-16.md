# 组合推送的仓库目标绑定

日期：2026-10-16。对应 DT-09/DT-10，延续[共享结束状态](desktop-git-workflow-close-2026-10-16.md)。

## 已实现

- 组合提交前读取规范化 worktree/Git 目录、当前分支、上游远端与目标分支、实际推送地址集合；提交后重新核对配置。推送地址只保存有序 SHA-256 指纹，原始 URL（可能含凭据）不写入目标描述。新增 sha2 使用 workspace 已有版本，没有新增版本来源。
- 仅组合提交为 Git 子进程设置本次 operationId 对应的 `GIT_REFLOG_ACTION`，不修改全局环境或仓库配置。成功后最多读取该分支 64 条 reflog（输出内容超过 256 KiB 拒绝），要求唯一匹配并核实当前 HEAD 仍等于本次提交 OID。另一个提交移动 HEAD、hook 继承标记形成多个匹配、reflog 缺失/关闭等，均不猜测来源提交。
- Git 成功回执新增可选 target/targetError。提交已成功但无法绑定推送目标时，仍保存 succeeded/completed=true，另记录推送目标待核实原因；返回给原提交调用的布尔语义不变，不因此再次提交。
- 后续 push/失败重试从根提交回执读取目标，重新核对当前授权目录、Git 目录、分支、HEAD、上游引用及推送地址指纹。任一变化时原接续失败，要求核实后独立发起推送。旧的无目标组合记录不自动按当前仓库回填目标。
- 实际推送显式指定原提交 OID 与原目标引用、当次核实的 URL，关闭自动跟随 tag 和子模块递归推送；后续 HEAD 移动不会改变命令中的源 OID。相对本地远端地址按原 worktree 解析。
- 支持最多 16 个推送 URL，顺序执行；部分失败保存已完成目标数量，重试仍发送原 OID。错误输出替换已知 URL 并经过已有脱敏器。普通独立 push/pull/sync 入口保持既有行为。
- 自定义 `remote.*.push`、镜像推送、与上游不同的 pushRemote 等覆盖配置不猜测目标，提交结果保留，提示核实后使用独立推送入口。当前绑定范围为正常上游分支推送，包括同一远端的多个 push URL。
- 操作详情/恢复列表显示“提交已完成，推送目标待核实”，不标提交失败或部分提交，不再提供该未绑定父操作的继续推送按钮。

依据：[Git 环境变量 GIT_REFLOG_ACTION](https://git-scm.com/docs/git#Documentation/git.txt-GITREFLOGACTION)、[git-push refspec](https://git-scm.com/docs/git-push)。原生测试使用本地普通/裸仓库，无公网凭据或真实 Agent。

## 验证

- `cargo test -p desktop --lib review -- --nocapture` 共 50 项通过，exit 0；日志 `/tmp/dogcodes-push-target-native.log`。包含原回执/关闭/断连恢复回归，以及本轮分支/HEAD/远端/上游引用变化拒绝、唯一提交识别、重复 marker、reflog 关闭、目标序列化无 URL 凭据、多地址部分失败与原提交重试、缺上游时提交成功仍保留。原工作流重启用例调整为提交前已配置目标、随后在同一地址恢复可用；不再通过事后更换推送配置放行旧工作流。
- 前端首轮 `ReviewOperationHistory`、`gitOperations`、`GitOperationRecovery` 22 项通过，类型检查通过；日志 `/tmp/dogcodes-push-target-frontend.log`、`/tmp/dogcodes-push-target-types.log`。
- 随后补充未绑定目标提示/隐藏继续按钮及其用例，针对两个受影响组件复验 12 项全部通过，最终类型检查通过，exit 0；日志 `/tmp/dogcodes-push-target-frontend-final.log`、`/tmp/dogcodes-push-target-types-final.log`。本轮共覆盖 23 个不同前端用例。原生测试后仅将只供测试调用的旧 commit_changes 包装函数加上 cfg(test)，实际提交使用 tagged 实现。所有测试句柄已收取终态，空白检查通过。
- 未执行全量前端、真实模型或打包。

## 仍需完成的发布验证

- 真正退出 GUI/后台进程后的恢复、Windows/Linux 实机、Git hooks/SSH/代理/凭据及远端保护规则，在候选安装版验证。新服务实例与本地裸仓库不能替代这些条件。
- 同项目仍允许外部 Git/Agent 并发修改，没有新增项目单写者锁。目标检查和 Git 外部进程不是跨进程原子事务；显式 OID/refspec 防止源分支后续移动改变发送内容，不宣称可阻止用户 hook 或同时修改 Git URL 重写配置的所有副作用。
- journal/父链/目录枚举长期增长和极大 Git 输出成本仍需性能验收；本轮没有清理或迁移用户历史数据。
- 下一工作块按入口矩阵核对独立于 CoreCommand 的审查、侧边问答与 Fork API 的请求身份、载荷绑定、丢失回复及重启恢复；DT-08/09/10 及发布候选尚未整体完成。
