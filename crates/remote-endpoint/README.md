# DogCodes endpoint security

共用 Rust 端點庫，規範源位於此 crate。PC 直接使用安全 API（停用預設 `ffi` feature）；App 在獨立倉庫保存同版本源碼快照，以 `tool/sync_endpoint.py` 同步並記錄逐文件 SHA-256。兩工程不依賴開發機絕對路徑。

提供 Snow Noise XXpsk3 首次配對、雙公鑰 pin 的 XX 重連、順序加密與重放拒絕；配對 journal 支援持久完成/撤銷/revision lease、競爭消費及回執核實。落盤不確定時停止授權，不自動清空身份。C ABI 僅供 App FFI 使用；官方路由服務不連結此 crate。

PC workspace 驗證：

```sh
cargo test -p dogcodes-endpoint --locked
cargo clippy -p dogcodes-endpoint --all-targets --locked -- -D warnings
```

9 項測試包含狀態/持久化與公開合成 Cacophony 向量。向量來源記於 fixture，只在測試使用固定 ephemeral。當前 PC 網關/手機 App 已接入；iOS/Android 模擬器的打包加密庫與系統安全存儲測試通過，真機及 Windows/Linux PC keyring 仍待驗收。完整實施證據見 `docs/implementation/mobile-server-framework-2026-10-18.md`（PC 工程根目錄）。

上層必須先保存配對完成再發加密 ACK，對每次請求/交付驗證 lease；身份只能來自已認證通道。底層回執核實已提供，但首次配對 ACK 丟失的在線查詢尚未接入。端點庫與整合測試不是獨立安全審計。
