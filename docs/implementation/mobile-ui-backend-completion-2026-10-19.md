# 依手機 UI 補齊 PC 與服務端能力

使用者於 2026-10-19 授權依 `../dogcodes-app/docs/design/mobile-2026-10-18` 的十二個畫面補齊兩端後端。本輪不重畫或重做手機頁面。尚無推送憑據，已確認先完成可配置代碼及本地驗證，真機通知待配置後驗收。桌面修改留在 `remote-control` 工作樹，main 保持乾淨。

本文為後端交付清單；「代碼接入」不能替代實機驗收。手機頁面本輪未改，目前仍走 WSS，還需要 Flutter DataChannel、選路與恢復狀態機。

| UI 能力 | 目前代碼 / 下一步 |
| --- | --- |
| 文件、Diff/Git、終端、項目運行 | 已接原 daemon 服務、回執及訂閱；真 Git / PTY / 運行記錄驗證通過。手機四頁待接入 |
| 會話重新命名/置頂/歸檔/刪除、Fork、歷史查詢 | 白名單與可信 PairedDevice 擴展，復用既有業務流程；多端真機驗收待做 |
| AI 審查、主 Agent 修復、原上下文 | 已開放原服務，修復 actor 持久化與舊記錄 LocalUser 兼容測試通過 |
| PC 既有目錄選取 / 加入項目 | 分頁查詢及帶設備身份的註冊，沿用原規範化及互斥；手機原生交互待接入 |
| 附件分塊上傳、核實/續傳/提交、分塊讀取 | 持久 offset/digest/uploadId、重啟續傳與衝突測試通過；項目文件先核實身份再導入，副本可查回執 |
| 屏幕/鍵鼠/逐屏切換 | 原生庫及有界工作線程、連接租約/序號、過期或斷線釋放；5fps JPEG 基礎接口。macOS 編譯與 Windows crate 交叉編譯通過；**高效媒體、持久捕獲與三平台原生權限/鎖屏/DPI實機驗收仍欠缺，不是 RustDesk 級媒體交付** |
| 手動文本/圖片剪貼板 | 已接原生讀寫；Linux 寫入保留 clipboard owner；待安全邊界及假後端測試 |
| 在線/最近在線、不同用途通道 | PG/Redis 狀態查詢與四類用途；跨節點認證/在線/用途測試通過 |
| 中繼限速/背壓 | **累積 64MiB 斷線已刪除**；按用途每分鐘令牌桶，等候原密文幀、繼續反向數據；暫滿有 2 秒背壓，持續不消費才斷開。65MiB 不重連、限速中雙向測試通過 |
| 網頁預覽 HTTP/WebSocket/HMR | 連接擁有、固定 loopback 端口、消息限額及序號；真 HTTP / WS 測試通過；手機 WebView 本地橋及 HMR 接入待完成 |
| 任務/遠控期間防自動睡眠，可關閉 | 全局持久版本化偏好、任務/review/run/terminal/遠控生命週期；偏好回執/版本/重啟測試通過。不可阻止手動睡眠或鎖屏 |
| 推送註冊/偏好/按電腦與會話靜音/派送 | PC 持久事件游標、PG outbox 去重/租約/退避，APNs/FCM/HMS/EMAS；假 adapter 測試去重/靜音/撤銷/失敗重試/目標失效通過；憑據與真機待驗收 |
| 配對回執丟失、綁定撤銷同步 | 限時恢復房間僅路由；固定公鑰新 Noise 後核實 PC 持久回執，才輪換有效 routeToken。雲端/本地撤銷同步，補登記不能復活撤銷；手機 pending 上下文持久化待接入 |
| 直連 / TURN / WSS | ICE、短期 TURN REST、`direct_open` 信令、獨立 Noise、選路診斷、回退入口已接入；真兩端 DataChannel 與完整加密網關驗證通過。手機自動選路與 iOS/Android 模擬器 P2P/回退已完成；PC TURN UDP/TCP/TLS 已強制中繼驗收，官方跨 NAT/切網仍待實測 |
| 手機草稿/快取 | 端內功能，這次 PC/服務端核對不改其端內所有權；查詢/版本/刪除事件需滿足同步 |

測試約束：不驅動真實鍵鼠、不截取使用者桌面作測試素材、不調用付費 Agent、不發真實推送。服務端使用 Docker PG/Redis；現有 development 資料庫及正在運行的舊容器不得為測試重設，migration 驗證使用獨立臨時資料庫。各平台正式實機驗收後才能標記遠控整包完成。

## 證據與接續

Core 87 通過；desktop 全量 252 通過、7 項原有真實環境測試忽略；endpoint 8 項安全 + 1 項固定向量、真 WebRTC 二進制順序/雙向通訊測試通過。server 22 通過（3 單元 + 19 HTTP/WS），含兩節點與持久儲存、65MiB、限速反向通訊、通知與配對恢復。不使用真實模型、推送、鍵鼠或屏幕素材。

早期兩項存儲測試曾向本機 development DB 執行加法 schema 遷移，未清空資料；後續使用專用 `dogcodes_backend_test_20261019`。本機 server 容器已換新並健康，保留 PG/Redis 卷；正式雲端未部署。服務 schema 3，啟動只按版本遷移，不反覆 ALTER TABLE。

精確傳輸、恢復、推送和部署契約在 server 的 `docs/transport-and-mobile-backend.md`；workspace 參見[網關接口](remote-workspace-gateway-2026-10-19.md)。先接入手機 DataChannel/選路狀態機和新增頁面 API，再驗收實網、原生媒體與推送。xcap 的 Wayland 截圖 fallback 可能經 Screenshot portal 交互，正式無人值守還需要持久 ScreenCast/RemoteDesktop portal 及授權恢復，不能把目前逐幀接口標成完成。手機與 PC 草稿仍各自持有。

完整 Dart → 中繼 → PC Core 聯調通過配對、發送、流式、審批、停止、額度、重連與撤銷（隔離合成 Agent）。該聯調曾發現 remote execute 經本地 RPC 會覆寫 actor 的回執衝突；已改為可信設備直接執行，任務刪除抽出共用清理服務，新增執行 / 回執同 actor 的回歸測試。主分支保持乾淨。
