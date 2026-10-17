<!-- Keep feature scope aligned across README.md and docs/i18n/. Screenshots are shared PNG files. -->
<div align="center">

<img src="../../apps/desktop/src-tauri/icons/128x128.png" alt="dogcodes" width="96" height="96" />

# dogcodes

**여러 코딩 에이전트를 하나의 작업 공간에서.**

<a href="../../README.md">English</a> · <a href="../../docs/i18n/README.zh-CN.md">简体中文</a> · <a href="../../docs/i18n/README.zh-TW.md">繁體中文</a> · <a href="../../docs/i18n/README.ja.md">日本語</a> · <strong>한국어</strong> · <a href="../../docs/i18n/README.es.md">Español</a> · <a href="../../docs/i18n/README.fr.md">Français</a> · <a href="../../docs/i18n/README.de.md">Deutsch</a> · <a href="../../docs/i18n/README.pt-BR.md">Português (Brasil)</a>

</div>

dogcodes은 코딩 에이전트, 대화, 코드 변경 사항, 프로젝트 실행 명령을 하나의 데스크톱 앱에 모아 줍니다. 한 에이전트로 기능을 구현하고 다른 에이전트로 검토한 뒤, 같은 작업 공간에서 프로젝트를 실행하고 커밋을 준비하세요.

**개발 프리뷰 · macOS 우선.** 설치 패키지와 릴리스 검증을 준비 중입니다. 현재 소스에서 실행할 수 있으며 Windows 검증은 아직 완료되지 않았습니다.

[기능](#features) · [시작하기](#getting-started)

## 지원 에이전트

이미 사용 중인 에이전트를 연결하세요. 아래 12개 연동이 구현되어 있습니다. 모델, 승인, 사용량 정보, 복구 기능은 에이전트와 설치 버전에 따라 다릅니다. 먼저 각 에이전트의 공식 도구로 설치하고 로그인하세요.

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

## 개발 흐름을 한곳에서

<!-- 스크린샷 자리: ../../docs/images/readme/overview.png -->

<a id="features"></a>

## 기능

### 에이전트 간 작업을 끊김 없이 이어받기

**같은 대화에서 실행할 에이전트를 언제든 바꿀 수 있으며, 컨텍스트는 자동으로 넘겨집니다.**

- 작업마다 가장 잘 맞는 에이전트에게 맡기세요. 설계는 설계에 강한 에이전트가, 세부 구현은 다른 에이전트가 맡습니다.
- 한 에이전트의 할당량이 바닥나도 요구사항을 다시 설명할 필요 없이 다른 에이전트가 이어서 작업합니다.
- 이전 대화가 함께 넘어가고, 이어받은 에이전트는 내장 히스토리 도구로 지난 대화도 찾아볼 수 있습니다. 처음부터 다시 하는 것이 아니라 정말로 이어서 진행합니다.

<!-- 스크린샷 자리: ../../docs/images/readme/switch-agent.png -->

### 할당량과 사용량을 한눈에

**지원하는 12가지 코딩 에이전트의 할당량, 사용량, 초기화 시각, 버전을 한곳에서 관리합니다.**

- 에이전트별 남은 할당량과 초기화 시각을 한눈에 보고 작업을 계획해 토큰을 알뜰하게 쓰세요.
- 에이전트 버전을 한곳에서 관리하고 클릭 한 번으로 업그레이드합니다.
- 설치된 에이전트와 로그인 상태를 자동으로 감지합니다.

<!-- 스크린샷 자리: ../../docs/images/readme/usage.png -->

### 이미 쓰고 있는 구독을 그대로

**설치하고 로그인해 둔 Claude Code, Codex, Cursor 등 공식 CLI를 내 요금제로 실행합니다.**

- 별도의 dogcodes 계정도, 추가 API 구매도 필요 없습니다. 이미 결제 중인 요금제를 그대로 씁니다.
- 로컬 우선: 데이터는 내 컴퓨터에 저장되며 중계 서버를 거치지 않습니다.
- 에이전트에는 꼭 필요한 환경 변수만 전달하므로 다른 비밀 정보가 노출되지 않습니다.

<!-- 스크린샷 자리: ../../docs/images/readme/accounts.png -->

### 에이전트 교차 코드 리뷰와 수정

**방금 변경한 코드를 다른 에이전트에게 리뷰받아 잠재적인 버그와 설계 결함을 줄이고, 보안·성능 분석도 할 수 있습니다.**

- 서로 다른 에이전트와 모델이 새로운 관점에서 교차 리뷰하여, 한 대화에 갇힌 단일 에이전트의 사각지대를 피합니다.
- 리뷰 결과는 높음·중간·낮음 우선순위로 나뉘며, 원하는 항목을 골라 클릭 한 번으로 수정을 시작합니다.
- 수정은 리뷰 보고서와 선택한 항목을 함께 담아 메인 에이전트에게 돌아가므로 컨텍스트가 끊기지 않습니다.

<!-- 스크린샷 자리: ../../docs/images/readme/review-fix.png -->

### 에이전트를 넘나드는 분기로 방식 비교

**원하는 턴에서 새 대화를 분기해 다른 에이전트가 다른 아이디어를 시도하게 할 수 있으며, 원래 대화는 그대로 남습니다.**

- worktree와 함께 쓰면 방식마다의 코드 변경이 서로 격리됩니다.
- 같은 문제를 여러 에이전트에게 각각 맡긴 뒤 가장 좋은 결과를 고르세요.

<!-- 스크린샷 자리: ../../docs/images/readme/fork.png -->

### 익숙한 방식의 Git 관리

**VS Code와 매우 비슷한 Git 조작 방식으로, 사이드바에서 평소처럼 Diff를 관리합니다.**

- 스테이지, 스테이지 취소, 폐기, 무시, Commit, Commit & Push를 지원하며 트리·목록 보기를 전환할 수 있습니다.
- 원하는 에이전트에게 클릭 한 번으로 커밋 메시지를 작성하게 하고 언어도 지정할 수 있습니다.

<!-- 스크린샷 자리: ../../docs/images/readme/git-changes.png -->

### 프로젝트 실행 방법을 분석하고 클릭 한 번으로 실행

**프로젝트의 실행·디버그 방법을 분석해 명령과 스크립트를 클릭으로 실행할 수 있는 항목으로 바꿔 줍니다.**

- 프로젝트마다 다른 시작 명령을 외우거나, 실행하려고 터미널을 따로 열 필요가 없습니다. 클릭 한 번이면 됩니다.
- 실행 출력은 한곳에 모여 언제든 확인할 수 있습니다.

<!-- 스크린샷 자리: ../../docs/images/readme/run-project.png -->

### 다중 폴더 프로젝트 지원

**프런트엔드, 백엔드, 문서가 서로 다른 폴더에 있나요? 하나의 대화에서 모두 에이전트에게 맡기세요.**

- 다중 폴더를 기본 지원하지 않는 에이전트도 dogcodes를 통하면 다중 폴더 프로젝트를 개발할 수 있습니다.

<!-- 스크린샷 자리: ../../docs/images/readme/multi-folder.png -->

### 창을 닫아도 작업은 계속

**에이전트 실행, 터미널, 프로젝트 명령은 백그라운드 서비스가 맡으므로 창이나 패널을 닫아도 중단되지 않습니다.**

- dogcodes를 다시 열면 같은 터미널과 출력에 자동으로 다시 연결됩니다.
- 연결이 끊기거나 예기치 않게 종료되어도 마지막 작업의 실제 결과를 확인할 수 있어, 커밋이나 푸시가 두 번 실행되지 않습니다.

<!-- 스크린샷 자리: ../../docs/images/readme/background.png -->

### 턴별 사용량을 한눈에

**턴마다 토큰 사용량, 컨텍스트 점유, 캐시 적중과 캐시 사용량을 확인할 수 있습니다.**

- 현재 대화와 각 턴의 소비를 자세히 파악하고, 토큰이 어디에 쓰였는지 알 수 있습니다.

<!-- 스크린샷 자리: ../../docs/images/readme/turn-usage.png -->

**오픈 소스 · 무료 · 크로스 플랫폼**: Apache-2.0 라이선스, Windows·macOS·Linux 지원.

<a id="getting-started"></a>

## 시작하기

현재는 소스에서 실행합니다. **Rust 1.97.1**, **Node.js 22+**, **pnpm 11** 및 [Tauri 플랫폼 필수 구성 요소](https://v2.tauri.app/start/prerequisites/)를 설치하세요. 저장소를 복제하거나 내려받은 뒤 루트에서 실행합니다.

```bash
pnpm install
pnpm desktop:dev
```

1. 지원 에이전트를 하나 이상 공식 절차로 설치하고 로그인합니다.
2. dogcodes을 열어 사용 가능한 에이전트를 연결하고 프로젝트 폴더를 추가합니다.
3. 새 대화에서 에이전트와 모델을 고르고 작업을 설명합니다.
4. 실행 패널에서 프로젝트 명령을 분석하거나 변경 사항을 확인하고 리뷰를 요청합니다.

대화, 프로젝트 분석, 리뷰, 커밋 메시지 생성은 선택한 에이전트의 계정을 사용하며 할당량을 소비할 수 있습니다.

<details>
<summary>개발 참여</summary>

```bash
cargo test --workspace
pnpm check
```

`pnpm dev`는 모의 데이터를 쓰는 브라우저 UI를 엽니다. 실제 에이전트와 프로젝트 명령에는 `pnpm desktop:dev`를 사용하세요.

</details>

## 자세히 알아보기

Pull Request 전에 [CONTRIBUTING.md](../../CONTRIBUTING.md)를 읽고, 보안 문제는 [SECURITY.md](../../SECURITY.md)에 따라 비공개로 제보해 주세요. 구현 및 릴리스 진행 상황은 [현재 작업 목록](../../docs/CURRENT_WORK.md), 원클릭 실행은 [프로젝트 실행 문서](../../docs/implementation/project-run-capabilities-2026-10-14.md)를 참고하세요.

페이지 구성은 [codex-host](https://github.com/BytePioneer-AI/codex-host)를 참고했습니다. 에이전트 아이콘은 저장소의 기존 자료를 사용합니다. [출처 및 고지](../../apps/desktop/src/assets/licenses/README.md)를 확인하세요. 제품 이름과 상표는 각 소유자에게 속합니다.
