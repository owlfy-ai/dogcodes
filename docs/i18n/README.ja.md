<!-- Keep feature scope aligned across README.md and docs/i18n/. Screenshots are shared PNG files. -->
<div align="center">

<img src="../../apps/desktop/src-tauri/icons/128x128@2x.png" alt="dog.codes" width="96" height="96" />

# dog.codes

**すべてのコーディングエージェントを、ひとつのワークスペースに。どこからでも指揮。**

<a href="../../README.md">English</a> · <a href="../../docs/i18n/README.zh-CN.md">简体中文</a> · <a href="../../docs/i18n/README.zh-TW.md">繁體中文</a> · <strong>日本語</strong> · <a href="../../docs/i18n/README.ko.md">한국어</a> · <a href="../../docs/i18n/README.es.md">Español</a> · <a href="../../docs/i18n/README.fr.md">Français</a> · <a href="../../docs/i18n/README.de.md">Deutsch</a> · <a href="../../docs/i18n/README.pt-BR.md">Português (Brasil)</a>

</div>

dog.codes は、コーディングエージェント、会話、コードの変更、プロジェクトの実行コマンドをまとめるデスクトップアプリです。ひとつのエージェントで実装し、別のエージェントでレビュー。そのままプロジェクトを実行してコミットを準備できます。席を離れても、スマートフォンからすべてを指揮できます。

[公式サイト](https://dog.codes) · [機能](#features) · [スマートフォン操作](#phone) · [はじめる](#getting-started)

## 対応エージェント

普段使っているエージェントを接続できます。以下の 12 種類との連携は実装済みです。モデル、承認、使用量データ、復旧機能はエージェントとバージョンによって異なります。先に公式ツールでインストールとログインを済ませてください。

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

## 開発の流れをひとつに

<!-- スクリーンショット予定: ../../docs/images/readme/overview.png -->

<a id="features"></a>

## 機能

### エージェント間でタスクをシームレスに引き継ぎ

**同じ会話のまま実行するエージェントをいつでも切り替え、コンテキストは自動で引き継がれます。**

- 作業ごとに最適なエージェントへ。設計は設計が得意なエージェントに、実装の細部は別のエージェントに任せられます。
- あるエージェントのクォータが尽きても、要件を説明し直すことなく別のエージェントが作業を続けます。
- これまでの会話が引き継がれ、引き継いだエージェントは組み込みの履歴ツールで過去のやり取りも参照できます。最初からやり直すのではなく、本当に続きから進みます。

<!-- スクリーンショット予定: ../../docs/images/readme/switch-agent.png -->

### クォータと使用量をひと目で把握

**対応する 12 種類のコーディングエージェントのクォータ、使用量、リセット時刻、バージョンをまとめて管理します。**

- 各エージェントの残りクォータとリセット時刻がひと目でわかり、作業を計画してトークンを無駄なく使えます。
- エージェントのバージョンを一元管理し、ワンクリックでアップグレードできます。
- インストール済みのエージェントとサインイン状態を自動で検出します。

<!-- スクリーンショット予定: ../../docs/images/readme/usage.png -->

### 今あるサブスクリプションをそのまま活用

**インストールしてサインイン済みの Claude Code、Codex、Cursor などの公式 CLI を、あなた自身のプランで動かします。**

- dog.codes 専用のアカウントも追加の API 購入も不要。すでに契約しているプランがそのまま使えます。
- ローカルファースト：コード、会話、認証情報はコンピューターの外に出ません。任意で使えるスマートフォン接続はエンドツーエンドで暗号化されるため、中継がデータを見ることはありません。
- エージェントには必要な環境変数だけを渡すため、ほかのシークレットがエージェントに漏れることはありません。

<!-- スクリーンショット予定: ../../docs/images/readme/accounts.png -->

### エージェント間のコードレビューと修正

**変更したばかりのコードを別のエージェントにレビューさせ、バグや設計上の欠陥を減らします。セキュリティやパフォーマンスの分析も可能です。**

- 異なるエージェント・異なるモデルが新しい視点で相互にレビューし、ひとつの会話の中に閉じた単一エージェントの死角を避けます。
- 指摘は高・中・低の優先度に分類され、選んだ項目をワンクリックで修正に回せます。
- 修正はレビュー結果と選んだ指摘を添えてメインのエージェントに戻されるため、コンテキストが途切れません。

<!-- スクリーンショット予定: ../../docs/images/readme/review-fix.png -->

### エージェントをまたいで分岐し、アプローチを比較

**任意のターンから新しい会話を分岐させ、別のエージェントに違うアイデアを試させられます。元の会話はそのままです。**

- worktree を使えば、アプローチごとのコード変更を互いに分離できます。
- 同じ課題を複数のエージェントにそれぞれ解かせ、良いものを採用できます。

<!-- スクリーンショット予定: ../../docs/images/readme/fork.png -->

### 使い慣れた感覚の Git 管理

**VS Code に近い Git 操作で、サイドバーからいつもどおりに Diff を管理できます。**

- ステージ、ステージ解除、破棄、無視、Commit、Commit & Push。ツリー表示とリスト表示を切り替えられます。
- 任意のエージェントにワンクリックでコミットメッセージを書かせ、言語も指定できます。

<!-- スクリーンショット予定: ../../docs/images/readme/git-changes.png -->

### プロジェクトの実行方法を分析し、ワンクリックで実行

**プロジェクトの実行・デバッグ方法を分析し、コマンドやスクリプトをクリックで起動できる項目に変換します。**

- プロジェクトごとの起動コマンドを覚えたり、起動のためだけにターミナルを開いたりする必要はもうありません。
- 出力は一か所に集まり、いつでも確認できます。

<!-- スクリーンショット予定: ../../docs/images/readme/run-project.png -->

### 複数フォルダーのプロジェクトに対応

**フロントエンド、バックエンド、ドキュメントが別々のフォルダー？ ひとつの会話でまとめてエージェントに任せられます。**

- 複数フォルダーにネイティブ対応していないエージェントでも、dog.codes を通せば複数フォルダーのプロジェクトを扱えます。

<!-- スクリーンショット予定: ../../docs/images/readme/multi-folder.png -->

### ターンごとの使用量を明確に

**ターンごとのトークン使用量、コンテキスト使用率、キャッシュヒット、キャッシュ使用量を確認できます。**

- 現在の会話と各ターンの消費を詳しく把握し、トークンがどこに使われたかがわかります。

<!-- スクリーンショット予定: ../../docs/images/readme/turn-usage.png -->

**オープンソース · 無料 · クロスプラットフォーム**：Apache-2.0 ライセンス。macOS、Windows、Linux 向けに構築しています。

<a id="phone"></a>

## スマートフォンからエージェントを指揮する

**ノート PC は置いて出かけましょう。エージェントはコンピューターで働き続け、指揮権はスマートフォンの手元に。**

スマートフォンを一度ペアリングすれば、以下のすべてをどこからでも実行できます。コードとエージェントはコンピューターの中にとどまります。

<!-- スクリーンショット予定: ../../docs/images/readme/phone-remote.png -->

- **会話の続き、または新しい会話。** コンピューター上の会話を開いて続けるほか、新しいセッションも開始できます。エージェント、モデル、思考の強さを選び、出力をリアルタイムで確認し、セッションの検索・固定・アーカイブ・フォークも可能です。
- **承認と回答。** 権限リクエストやエージェントの質問がプッシュ通知とともにスマートフォンに届き、ワンタップでエージェントが再開します。承認はすべて、コンピューター上の元の実行と照合されます。
- **レビューとコミット。** 各プロジェクトフォルダーの変更を確認し、ファイルをステージして、エージェントにコミットメッセージを書かせ、コミット、プッシュ、プル、同期。コミット履歴の閲覧やブランチの比較（読み取り専用）もできます。
- **PC のフォルダーからプロジェクトを追加。** コンピューターのフォルダーを閲覧して、プロジェクトとして追加。デスクに戻る必要はありません。
- **PC のターミナルでコマンドを実行。** 任意のプロジェクトフォルダーでターミナルを開き、コンピューターの本物のシェルで作業できます。
- **プロジェクトを実行して、スマートフォンで確認。** コンピューターで起動項目を実行し、動作中の Web アプリをスマートフォンでそのまま開きます。
- **ファイルの閲覧と添付。** プロジェクトのファイルを読み、メッセージにファイルや画像を添付できます。
- **必要ならリモートデスクトップ。** コンピューターの画面を表示し、タップ、ドラッグ、スクロール、ピンチ、入力、テキストや画像の貼り付けが可能。画質はネットワークに合わせて自動調整されます。

<!-- スクリーンショット予定: ../../docs/images/readme/remote-desktop.png -->

### 接続の仕組み

- QR コードを一度スキャンするだけで、アカウントは不要です。通信はエンドツーエンドで暗号化されます。複数のコンピューターを登録でき、オンライン状況の確認や、スマートフォンの連携解除はデスクトップ側からいつでも可能です。
- スマートフォンは可能なら直接接続し、できない場合は dog.codes の中継サービスを経由します。中継は暗号化された通信を転送するだけです。
- コンピューターは電源を入れたまま、dog.codes を開いた（または最小化した）状態にしてください。作業やリモートセッション中は、dog.codes が自動スリープを防げます（オフにもできます）。
- リモートデスクトップは macOS、Windows、Linux（X11 / Wayland）のコンピューターに対応します。必要な画面・入力の権限は OS が確認します。

<a id="getting-started"></a>

## はじめる

現在はソースから実行します。**Rust 1.97.1**、**Node.js 22+**、**pnpm 11** と [Tauri のプラットフォーム依存環境](https://v2.tauri.app/start/prerequisites/)を用意してください。リポジトリをクローンまたはダウンロードし、ルートで実行します。

```bash
pnpm install
pnpm desktop:dev
```

1. 対応エージェントをひとつ以上、公式の手順でインストールしてログインします。
2. dog.codes を開き、利用可能なエージェントを接続してプロジェクトフォルダーを追加します。
3. 会話を作成し、エージェントとモデルを選んで作業内容を伝えます。
4. 実行パネルでコマンドを分析するか、差分を確認してレビューを依頼します。
5. スマートフォンからエージェントを指揮するには、デスクトップアプリで「スマートフォンを接続」を開き、スマートフォンアプリでペアリングコードをスキャンします。

会話、プロジェクト分析、レビュー、コミットメッセージの生成は、選択したエージェントのアカウントと利用枠を使用します。

<details>
<summary>開発に参加する</summary>

```bash
cargo test --workspace
pnpm check
```

`pnpm dev` は模擬データを使うブラウザ版 UI です。実際のエージェントやコマンドを使う場合は `pnpm desktop:dev` を実行してください。

</details>

## 詳しい情報

プルリクエストの前に [CONTRIBUTING.md](../../CONTRIBUTING.md) を読み、セキュリティ問題は [SECURITY.md](../../SECURITY.md) の手順で非公開に報告してください。

### エージェントの接続方式

dog.codes は各エージェントのローカルプロセスのインターフェースに接続します。ACP は Agent Client Protocol の略です。別のネイティブインターフェースを使うエージェントには専用ドライバーを用います。

| エージェント | 接続方式 | 対応するインストール元 |
| --- | --- | --- |
| Codex | ネイティブ `app-server`（JSON-RPC） | デスクトップクライアント + CLI |
| Claude Code | ネイティブ `stream-json` | デスクトップクライアント + CLI |
| Cursor | ACP（`cursor-agent acp`） | CLI |
| Kimi Code | ACP（`kimi acp`） | CLI |
| Grok Build | ACP（`grok agent stdio`） | CLI |
| ZCode | ネイティブ `app-server`（内蔵 `zcode.cjs`） | デスクトップクライアント（内蔵 CLI） |
| Antigravity | ネイティブのヘッドレス `stream-json`（`agy`） | CLI |
| CodeBuddy | ACP（`codebuddy --acp`） | CLI |
| Pi | ネイティブ RPC（`pi --mode rpc`） | CLI |
| OpenCode | ACP（`opencode acp`） | デスクトップクライアント + CLI |
| DeepSeek Harness | ACP（`dsh --profile acp`） | デスクトップクライアント + CLI |
| Kiro | ACP（`kiro-cli acp`） | CLI |

「デスクトップクライアント」は、そのクライアントが同梱またはインストールした CLI／ヘッドレス実行環境を再利用することを指し、元の GUI を自動操作する意味ではありません。対応する実行環境の導入が必要で、検出パスと利用可否は OS やバージョンによって異なります。OpenCode のデスクトップ内蔵実行環境の自動検出は、現在 Windows に実装されています。

### 謝辞

Computer use のウィンドウ情報取得、スクリーンショット、マウスとキーボードの操作には、MIT ライセンスの [Cua Driver](https://github.com/trycua/cua) を利用しています。デスクトップ自動化をオープンソースで提供する Cua チームと貢献者の皆さまに感謝します。現在、同梱ドライバーの統合は Windows x64 に対応しています。

エージェントのアイコンにはリポジトリ内の既存素材を使用しています。[出典と表記](../../apps/desktop/src/assets/licenses/README.md)をご確認ください。製品名と商標は各権利者に帰属します。
