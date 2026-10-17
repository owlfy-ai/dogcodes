# ZCode 后台运行与 Dock 图标

2026-09-21：接入使用 ZCode 安装包内的 `Resources/glm/zcode.cjs app-server`，通过 stdin/stdout 的 JSON 消息处理对话、审批与取消。并非通过操控 ZCode 窗口完成对话。

原启动方式为 `ELECTRON_RUN_AS_NODE=1 ZCode.app/Contents/MacOS/ZCode ...`。本机检查确认 dogcodes Core 的 `zcode-cli` 子进程被 NSRunningApplication 登记为 activationPolicy=0（普通应用），关联 ZCode.app。即使是 Node 模式，使用主应用可执行文件仍会产生额外的 Dock 应用项。

现在使用同一安装包内的 `Frameworks/ZCode Helper.app/Contents/MacOS/ZCode Helper`。该包声明 LSUIElement=true。实测 Node 24.14.0 / Electron 41.0.3，与内嵌运行时一致；`session/list` 协议握手成功，NSRunningApplication 返回 activationPolicy=1（辅助应用，不显示 Dock 图标）。

统一覆盖安装版本/身份检测、app-server 连接、额度读取及模型运行配置读取。安装身份仍是原 ZCode 安装路径。Helper 缺失或未声明后台属性时检测失败，不退回会干扰用户的主程序；不修改 ZCode 安装包，也不增加系统 Node 依赖。

增加了 Helper 属性验证与实际子进程路由回归，确认探测启动后台入口、缺少 Helper 时不启动前台程序。真实协议验证没有发起模型对话。现有已启动的旧后台进程随宿主退出，更新后重启 dogcodes 使用新入口。

验证：Runtime、Registry、ZCode Driver 默认测试共 36 项通过；新增进程路由测试再次通过。本机完整 readiness 探针通过（临时会话创建并关闭，无模型请求）；桌面原生构建通过。没有重启或关闭用户正在使用的 ZCode。
