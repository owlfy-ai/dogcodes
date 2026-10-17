# M1 稳定性收尾（2026-09-19）

## 范围

本次处理服务锁偶发失败、Rust 格式与严格 Clippy 告警。服务锁保护同一应用数据目录的 Core 服务所有权，不限制多个 Agent 或多个对话同时修改同一项目。Git 提交备注 AI 总结不增加执行留痕。

## 服务锁修复

原实现返回持锁的 `std::fs::File`，释放时仅依靠文件关闭。Unix 上 `dup` 或 `fork` 产生的描述符共享同一个 open file description；只关闭所有者的描述符时，其他副本可能继续持有锁。并行测试中的 PTY 启动会经过 fork/pre-exec，这能解释关闭后立即重新获取锁偶发失败的现象。

新增确定性回归测试：获取锁后用 `try_clone` 保留共享描述符，释放所有者并立即重新获取锁。旧实现稳定失败（`released owner still holds the lease`），修复后通过。原始偶发失败没有捕获进程描述符快照，因此这里确认的是可复现的锁生命周期缺陷，而非声称已追踪到原始失败瞬间的具体子进程。

`HostLease` 现在封装持锁文件，并在 `Drop` 时显式 `unlock`。锁文件路径和非阻塞获取方式不变；不删除或重建锁文件。正常释放不再等待继承描述符关闭。操作系统仍负责进程强杀后的锁回收；如果强杀时仍有继承副本存活，Unix 锁仍可能等到副本关闭才释放。

回归覆盖：

- 同一目录不能同时存在两个持锁所有者；释放后可重新获取。
- 共享描述符仍存活时，所有者释放仍允许重新获取；旧描述符关闭不会释放新所有者的锁。
- 独立子进程持锁时父进程获取失败；强杀并等待子进程退出后可获取。
- 桌面完整测试以 16 个测试线程连续执行 20 轮，每轮 53 项通过、2 项按既有规则忽略，未再出现服务锁失败。

实现：`apps/desktop/src-tauri/src/lib.rs` 的 `HostLease` / `acquire_host_lease`；测试：`apps/desktop/src-tauri/src/daemon.rs` 的三个 `service_lease_*` 测试。

## 格式和 Clippy

按工作区既有 lint 配置清理全部 target：显式导入、返回值标注、冗余转换和克隆、分支表达式、受检整数转换、私有函数参数分组等。Agent 扫描的内部大型 future 在超时边界装箱，避免上层调用链传播过大的 future。服务 runtime 初始化失败改为输出错误并正常返回。

未降低工作区 lint 等级。两个局部 `expect` 分别说明 Tauri 注入参数加命名 IPC 参数的固定命令签名，以及灵动岛四舍五入后转为整数物理像素的必要转换。

## 验证

环境：macOS，本机 Rust/Cargo 1.97.1。

- `cargo fmt --all -- --check`：通过。
- `cargo clippy --workspace --all-targets -- -D warnings`：通过。
- `cargo clippy -p desktop --all-targets --features review-acceptance -- -D warnings`：通过，覆盖应用直接持有服务锁的验收特性路径。
- `pnpm check`：通过，324 项前端测试、类型检查和生产构建；构建仍提示已有的大 chunk 建议。
- `cargo test -p desktop --lib --features review-acceptance`：56 通过、2 忽略、0 失败。
- 最终 `cargo test --workspace`：436 通过、61 忽略、0 失败；统计不重复计算回归测试内部启动的子进程测试。
- 首次完整回归遇到 Claude 测试 `reconfiguration_updates_a_warm_process_and_waits_for_matching_ack` 的进程关闭超时。单项复测通过；Claude 完整测试组以 16 个线程连续 5 轮通过（每轮 22 通过、6 忽略），最终完整工作区重跑也通过。该次 Claude 超时未稳定复现，保留为观察项，不宣称其根因已修复。

本次没有执行忽略的真实 Agent / 网络验收，也不代替 Windows、GUI、打包发布验收。M1 的 SDK、Hub 只读 Delegation 和剩余产品验收仍独立跟进。
