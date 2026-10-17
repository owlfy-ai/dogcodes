# 客户端升级兼容排查（2026-09-25）

## 本机现象与根因

- Codex 安装位于 `/Applications/ChatGPT.app`。Info.plist 的桌面版本为
  `26.917.71314`，同包 `Resources/codex --version` 返回
  `codex-cli 0.155.0-alpha.16.4`。二者都是当前真实版本。额度页一直使用安装记录中的
  CLI 版本，因此给人升级未生效的印象。
- ZCode 桌面版本为 `3.14.3`，内置 CLI 为 `0.16.9`。后台 Electron Helper 仍正常。
  `app-server` 在握手前退出：新入口寻找 `glm/provider/zcode-builtin.json` 或源码树目录，
  实际文件却在 `Resources/config/provider/zcode-builtin.json`。
- 补齐配置路径后继续发现协议变更：`workspace/readState` 返回 JSON-RPC `-32601`；
  `app-server` 不再执行 standalone CLI 的旧配置导入。本机 v2 provider registry 为空，
  原 `.zcode/cli/config.json` 的自定义 API-key 模型仍然存在。
- 新模型列表中的 reasoning 不再带 `enabled`，选择模型时必须在模型引用中携带
  `options.reasoningLevel`。省略时返回 `invalid_model_request`。

## 修复

- 额度页分别显示客户端版本和引擎版本。桌面版本独立从本机安装读取，远端更新检查失败时
  仍可返回；桌面版本参与更新缓存失效判断。引擎版本继续用于协议兼容判断。
- 从宿主更新窗口返回后重新检测该供应商，再强制刷新更新状态。
- ZCode 启动显式传入当前安装包的内置配置路径。
- 仅当原生 v2 registry 缺失或为空时，为现有旧 CLI 自定义 API-key 配置建立连接专用的
  临时兼容配置。文件权限为 `0600`，连接释放时删除，不覆盖原生配置，不输出凭据。
  已存在的非空原生 registry 优先；无法忠实转换的选项、账户型配置及旧格式加密 key
  明确报未配置，不将密文当作 key 或静默替换认证方式。
- 只有旧模型接口返回明确的 method-not-found 时，才回退到创建/关闭临时会话读取模型列表。
  readiness 同时检查可用模型，避免空 registry 被误报为已就绪。
- 保留供应商默认思考级别，并兼容升级前保存的不含 reasoning options 的模型选择。

## 验证

- ZCode driver 单元测试通过，覆盖配置路径、临时文件权限/删除、原生配置优先、模型引用等。
- 更新逻辑后端测试通过；前端更新、额度组件、布局测试共 17 项通过。
- 类型检查、前端生产构建、桌面构建通过；ZCode driver Clippy 检查通过。
- 真实 ZCode readiness：list → create → close → list 通过。
- 真实模型列表、选择模型/思考级别、升级前选择引用、关闭临时会话通过。
- 真实官方额度读取通过。
- 通过正在运行的 dogcodes Core RPC 确认 Codex 与 ZCode 均为 ready；ZCode 返回
  1 个可用模型及默认选择，运行服务构建与本机二进制一致。

未执行消耗模型额度的聊天、审批或跨进程历史恢复验收。0.16.9 的冷恢复能力保持 Unknown，
没有沿用 0.16.5 的已验证标签。
