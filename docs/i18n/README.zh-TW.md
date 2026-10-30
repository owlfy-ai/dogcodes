<!-- Keep feature scope aligned across README.md and docs/i18n/. Screenshots are shared PNG files. -->
<div align="center">

<img src="../../apps/desktop/src-tauri/icons/128x128@2x.png" alt="dog.codes" width="96" height="96" />

# dog.codes

**所有 Coding Agent，一個工作台，隨時隨地指揮。**

<a href="../../README.md">English</a> · <a href="../../docs/i18n/README.zh-CN.md">简体中文</a> · <strong>繁體中文</strong> · <a href="../../docs/i18n/README.ja.md">日本語</a> · <a href="../../docs/i18n/README.ko.md">한국어</a> · <a href="../../docs/i18n/README.es.md">Español</a> · <a href="../../docs/i18n/README.fr.md">Français</a> · <a href="../../docs/i18n/README.de.md">Deutsch</a> · <a href="../../docs/i18n/README.pt-BR.md">Português (Brasil)</a>

</div>

dog.codes 將程式開發 Agent、對話、程式碼變更與專案執行指令整合在同一個桌面應用程式。讓一個 Agent 實作功能，換另一個審查，再執行專案、準備提交，都能在同一個工作空間完成。離開辦公桌後，還能用手機繼續指揮這一切。

[官網](https://dog.codes) · [功能](#features) · [手機遙控](#phone) · [開始使用](#getting-started)

## 支援的 Agent

連接你已經使用的程式開發助手。以下 12 個 Agent 均已整合；可用模型、核准流程、用量資料與恢復能力會依 Agent 及安裝版本而異。請先透過各 Agent 的官方工具完成安裝與登入。

<p align="center">
  <a href="https://developers.openai.com/codex/"><img src="../../docs/images/readme/agents/codex.png" alt="Codex" height="36" /></a>
  <a href="https://code.claude.com/docs/en/overview"><img src="../../docs/images/readme/agents/claude-code.png" alt="Claude Code" height="36" /></a>
  <a href="https://cursor.com/"><img src="../../docs/images/readme/agents/cursor.png" alt="Cursor" height="36" /></a>
  <a href="https://www.kimi.com/"><img src="../../docs/images/readme/agents/kimi.png" alt="Kimi Code" height="36" /></a>
  <a href="https://grok.com/"><img src="../../docs/images/readme/agents/grok.png" alt="Grok Build" height="36" /></a>
  <a href="https://z.ai/"><img src="../../docs/images/readme/agents/zcode.png" alt="ZCode" height="36" /></a>
  <br />
  <a href="https://antigravity.google/"><img src="../../docs/images/readme/agents/antigravity.png" alt="Antigravity" height="36" /></a>
  <a href="https://www.codebuddy.ai/"><img src="../../docs/images/readme/agents/codebuddy.png" alt="CodeBuddy" height="36" /></a>
  <a href="https://pi.dev/"><img src="../../docs/images/readme/agents/pi.png" alt="Pi" height="36" /></a>
  <a href="https://opencode.ai/"><img src="../../docs/images/readme/agents/opencode.png" alt="OpenCode" height="36" /></a>
  <a href="https://github.com/deepseek-ai/deepseek-harness"><img src="../../docs/images/readme/agents/deepseek-harness.png" alt="DeepSeek Harness" height="36" /></a>
  <a href="https://kiro.dev/"><img src="../../docs/images/readme/agents/kiro.png" alt="Kiro" height="36" /></a>
</p>

## 把開發流程放在一起

<!-- 截圖預留: ../../docs/images/readme/overview.png -->

<a id="phone"></a>

## 用手機指揮你的 Agent

**出門不必帶電腦。Agent 在電腦上照常幹活，手機始終掌握指揮權。**

手機只需配對一次，下面的一切都能在任何地方完成。你的程式碼和 Agent 始終留在自己的電腦上。

<!-- 截圖預留: ../../docs/images/readme/phone-remote.png -->

- **接續對話或發起新對話。** 接著電腦上的任一對話繼續，也可以新增工作階段：選擇 Agent、模型和思考強度，即時查看串流輸出，還能搜尋、置頂、封存或分岔工作階段。
- **批准權限，回答提問。** 權限請求和 Agent 的提問會連同推播通知送到手機，點一下 Agent 就繼續工作。每次批准都會與電腦上原本的執行核對。
- **審查並提交。** 查看每個專案目錄的變更，暫存檔案，讓 Agent 寫好提交說明，再提交、推送、拉取或同步。還能唯讀瀏覽提交歷史、比較分支。
- **從電腦目錄建立專案。** 瀏覽電腦上的資料夾，選一個加入為專案，不必坐回桌前。
- **在電腦終端機裡執行指令。** 在任一專案目錄開啟終端機，使用你電腦上真實的 Shell。
- **執行專案，在手機上看效果。** 在電腦上啟動執行項，手機裡直接開啟正在執行的網頁應用程式。
- **瀏覽檔案，傳送附件。** 查看專案檔案，並在訊息中附上檔案和圖片。
- **需要時開啟遠端桌面。** 查看電腦螢幕，點按、拖曳、捲動、捏合縮放、輸入，或貼上文字和圖片，畫質會隨網路狀況自動調整。

<!-- 截圖預留: ../../docs/images/readme/remote-desktop.png -->

### 連線方式

- 只需掃一次 QR code，無須帳號。連線全程端對端加密。可配對多台電腦、查看上線狀態，並隨時在桌面端撤銷某支手機。
- 手機能直連時直連，否則回退到 dog.codes 中繼服務，中繼只轉送加密資料。
- 電腦需保持開機，dog.codes 保持開啟或最小化。有任務或遠端工作階段進行時，dog.codes 可以防止電腦自動睡眠，也可以關閉此選項。
- 遠端桌面支援 macOS、Windows 和 Linux（X11 與 Wayland）電腦；系統會要求所需的螢幕與輸入權限。

<a id="features"></a>

## 功能

### 多 Agent 無縫接續任務

**同一個對話中可隨時更換執行的 Agent，自動交接上下文。**

- 隨時把工作交給最適合的 Agent：擅長架構的負責設計，擅長細節的負責實作。
- 某個 Agent 額度用盡，換另一個 Agent 接著做，不必重新說明需求。
- 交接時會帶上先前的對話內容，接手的 Agent 也能透過內建歷史工具查閱過往紀錄，是真正接著做，而不是重新開始。

<!-- 截圖預留: ../../docs/images/readme/switch-agent.png -->

### 隨時掌握額度與用量

**集中管理 12 種 Coding Agent 的額度、用量、重置時間與版本。**

- 每個 Agent 的剩餘額度和重置時間一目了然，方便安排工作，把每一份 token 都用足。
- 統一管理各 Agent 的版本，一鍵升級。
- 自動發現本機已安裝的 Agent，並偵測登入狀態。

<!-- 截圖預留: ../../docs/images/readme/usage.png -->

### 沿用你現有的訂閱，不必另購 API

**直接調用本機已登入的 Claude Code、Codex、Cursor 等官方命令列工具，用的是你自己的訂閱額度。**

- 不需要另外的 dog.codes 帳號，也不必另外購買 API，已有的訂閱直接派上用場。
- 本機優先：程式碼、對話和憑證都留在你的電腦上。可選的手機連線全程端對端加密，中繼看不到你的資料。
- 啟動 Agent 時只傳入必要的環境變數，你的其他密鑰不會暴露給 Agent。

<!-- 截圖預留: ../../docs/images/readme/accounts.png -->

### 多 Agent 程式碼審查並修復

**指定另一個 Agent 審查剛修改的程式碼，減少潛在 bug 與設計缺陷，也可進行安全與效能分析。**

- 不同 Agent、不同模型交叉評審，跳出原本的上下文換個視角，避免單一 Agent 在同一上下文中的盲點。
- 評審結果分為高、中、低優先級，選中問題後一鍵「開始修復」。
- 修復交回主 Agent 執行，並附上審查報告與選中的問題，上下文連貫，改得更準。

<!-- 截圖預留: ../../docs/images/readme/review-fix.png -->

### 跨 Agent 分叉，平行比較方案

**從任意一輪分叉出新對話，交給另一個 Agent 嘗試不同思路，原對話不受影響。**

- 配合 worktree，不同方案的程式碼改動彼此隔離。
- 讓不同 Agent 對同一問題各出一版，比較後擇優採用。

<!-- 截圖預留: ../../docs/images/readme/fork.png -->

### 符合使用習慣的 Git 管理

**高度貼近 VS Code 的 Git 操作邏輯，在側邊欄用熟悉的方式管理 Diff。**

- 暫存、取消暫存、捨棄、忽略，Commit 及 Commit & Push，樹狀與清單檢視自由切換。
- 可指定任一 Agent 一鍵產生 Commit Message，並可指定語言。

<!-- 截圖預留: ../../docs/images/readme/git-changes.png -->

### 自動分析專案運行方式，一鍵執行

**自動分析專案的運行、除錯命令或腳本，轉成可點擊的快速啟動項。**

- 不用再記不同專案各自的啟動命令，也不必專門打開終端機，點一下就能執行。
- 運行輸出集中顯示，隨時查看。

<!-- 截圖預留: ../../docs/images/readme/run-project.png -->

### 多目錄專案支援

**前端、後端、文件分散在不同目錄？一個會話全部交給 Agent。**

- 原本不支援多目錄的 Agent，經 dog.codes 管理後也能進行多目錄專案開發。

<!-- 截圖預留: ../../docs/images/readme/multi-folder.png -->

### 最小化視窗，工作繼續

**桌面端保持開啟或最小化時，任務、終端機、專案指令和手機連線持續執行。關閉主視窗或結束應用程式會停止 Core 及其工作。**

桌面端採用單一執行個體，重複啟動會聚焦現有視窗。有活動任務時關閉需要確認；中斷的指令不會自動重送。

- 有任務、審查、執行、終端機或遠端工作階段進行時，dog.codes 可以防止電腦自動睡眠（可關閉）。它無法阻止你闔上螢幕或鎖定畫面。

<!-- 截圖預留: ../../docs/images/readme/background.png -->

### 每一輪的用量明細一目了然

**每一輪的 token 消耗、上下文佔用、快取命中與快取用量清清楚楚。**

- 詳細掌握目前會話及每一輪對話的消耗，知道 token 花在哪裡。

<!-- 截圖預留: ../../docs/images/readme/turn-usage.png -->

### 每次傳送、提交和推送只發生一次

**dog.codes 在執行每個操作前先記錄，重試前先核實結果。**

- 應用程式關閉或連線在操作中途中斷時，它會核實實際發生了什麼，而不是猜測——不會重複傳送訊息、重複提交或重複推送。
- 中斷的指令不會自動重送。

### 介面使用你的語言

**dog.codes 提供 11 種介面語言——English、简体中文、繁體中文、日本語、한국어、Español、Français、Deutsch、Português (Brasil)、Русский 和 Italiano——也可以跟隨系統語言。**

**開源 · 免費 · 跨平台**：Apache-2.0 授權，面向 macOS、Windows 和 Linux 建置。

<a id="getting-started"></a>

## 開始使用

目前透過原始碼執行。請先安裝 **Rust 1.97.1**、**Node.js 22+**、**pnpm 11** 及 [Tauri 平台相依套件](https://v2.tauri.app/start/prerequisites/)。複製或下載此儲存庫後，在根目錄執行：

```bash
pnpm install
pnpm desktop:dev
```

1. 依官方方式安裝至少一個支援的 Agent 並登入。
2. 開啟 dog.codes，連接可用 Agent，加入專案目錄。
3. 建立對話，選擇 Agent 和模型，描述要完成的工作。
4. 開啟執行面板分析專案指令，或檢視變更並發起審查。
5. 若要用手機指揮 Agent，請在桌面端開啟「連接手機」，並用手機端掃描配對碼。

Agent 對話、專案分析、審查與提交訊息產生會使用所選 Agent 的帳號，可能消耗其額度。

<details>
<summary>參與開發</summary>

```bash
cargo test --workspace
pnpm check
```

`pnpm dev` 啟動使用模擬資料的瀏覽器介面。使用真實 Agent 與執行專案指令，請執行 `pnpm desktop:dev`。

</details>

## 更多資訊

提交 Pull Request 前請閱讀 [CONTRIBUTING.md](../../CONTRIBUTING.md)；安全問題請依 [SECURITY.md](../../SECURITY.md) 私下回報。

Agent 圖示沿用儲存庫現有素材，來源與聲明請見[圖示說明](../../apps/desktop/src/assets/licenses/README.md)。產品名稱及商標屬於各自擁有者。
