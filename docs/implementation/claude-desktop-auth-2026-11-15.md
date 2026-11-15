# Claude Desktop 登录优先（2026-11-15）

DogCodes 的 Claude Code 原生运行程序可以来自独立 CLI 或桌面客户端下载的运行时。
登录来源与可执行文件来源独立：支持 host-managed provider/host refresh 的运行时
（本次核对的最低版本为 2.1.325）启动时先读 Claude Desktop 当前账户的有效 Code
凭证；不可用时保留原先 CLI 配置、Shell 环境及原生登录/续期行为。旧版或版本未知的
运行时不注入桌面令牌，避免旧版 settings.env 把令牌转发到自定义 API。

## 选择和回退

- 仅读取桌面 `config.json` 的 `oauth:tokenCacheV2`，通过当前系统用户的
  Electron safeStorage 解密；从不读取或导出聊天 Cookie。
- 账户必须匹配 `lastKnownAccountUuid`，OAuth client 必须匹配原生 Code client，
  API host 固定为 `https://api.anthropic.com`，并有 inference/profile/file_upload scope。
  优先采用桌面本地 everyday Code grant，再用带 sessions scope 的 Code grant。
- 不使用聊天 client 的 token、其他账户、过期/即将过期 token；多个组织无法确定时
  回退 CLI，不猜测活跃组织。多个桌面安装目录的身份不一致时也回退。
- 桌面凭证选中后，以原生 host-managed provider guard 固定官方服务和认证环境，
  隔离 Shell/API key/自定义 endpoint；不更改 CLI 的配置目录和持久登录。
- 原生模型或额度查询明确拒绝某个桌面 bearer 后，本次 DogCodes 进程记住该拒绝。
  后续启动回退 CLI；桌面出现新的 bearer 后可再次优先选择桌面。
  不自动重放用户请求，不在进行中的任务内切换账户或切到 CLI。

## 续期边界

Desktop 仍拥有自己的登录和续期链。DogCodes 不消费它的 refresh token，
不修改 token cache，不自行通过桌面 Cookie 签发 grant，也不接管客户端的 OAuth。
原生 `oauth_token_refresh` 回调只返回桌面已更新的、同一账户/同一组织/同一 scope
的有效 bearer。回调在进入时间线和工具错误处理前截获，DogCodes 不将令牌写入日志、
事件、数据库、临时文件或进程参数。业务会话读取凭证期间不持有 stdin 锁，中断仍可发送。

因此，仅安装客户端、仅登录 Chat、客户端缓存尚无 Code grant、系统拒绝读取，
或客户端没有维持有效 Code grant 时，不能保证借用桌面登录；这些情况使用 CLI。
如果两边都没有可用登录，仍需用户在原生产品完成登录。这个桥接不消除服务端
登录有效期限制，也不保证关闭客户端后仍能无限续期。

## 系统存储

- macOS：`~/Library/Application Support/Claude`，Keychain `Claude Safe Storage` /
  `Claude Key`，Electron v10 AES-CBC。首次读取可能出现系统钥匙串授权；拒绝、
  取消或 8 秒超时回退 CLI，不修改钥匙串 ACL。
- Windows：Roaming 及 Claude MSIX LocalCache，Local State 中 DPAPI 保护的
  Electron v10 AES-GCM key；通过固定 PowerShell/.NET CurrentUser DPAPI helper
  在内存解密，不引入 PowerShell profile 或新依赖安装。
- Linux：`$XDG_CONFIG_HOME/Claude` 或 `~/.config/Claude`，GNOME Secret Service
  的 `secret-tool` / Electron v11；缺少 helper、其他 keyring 或未知加密格式回退 CLI。
- 远程执行不读取本机桌面凭证，继续使用远端自己的 CLI 登录。
- JSON/pipe/时间都有上限，密钥只短期保留于内存，失败结果也有短期缓存。

## 验证

离线 fixture 覆盖真实 Electron CBC/GCM 格式、当前账户和组织选择、everyday 优先、
桌面覆盖 CLI、过期/拒绝后 CLI 原样回退、新桌面登录重新优先、远程隔离、原生
refresh 回调与切换账户后拒绝续接。测试使用私有目录和合成 token，不访问真实
凭证，不调用模型。86 个离线测试和 macOS Clippy 通过，Windows MSVC 全目标
编译检查通过。Linux 交叉检查因本机没有 GLib/sysroot 的 pkg-config 配置而未完成。
另用隔离 HOME、合成令牌验证了本机原生 2.1.325 的 initialize/get_settings 控制通道；
没有执行真实账户认证或模型请求。编译检查不能替代系统存储实机验收。

本实现依据本机 Claude Desktop/Claude Code 2.1.325 的行为；桌面 token cache 和
host refresh 均为原生内部接口，格式变化时应保守回退 CLI。系统存储格式参考
[Electron safeStorage](https://www.electronjs.org/docs/latest/api/safe-storage)。
