# Harness integration follow-up

Reference: local codex-host `7cc4db8` (v0.7.0), including `44cb200`, #253, #255, #211 and #254. dogcodes retains its Rust driver/Core boundaries; no Host/Electron internals were imported.

## Antigravity approval audit

The current upstream removed ordinary tool approval Hooks. dogcodes deliberately retains its existing explicit `desktop-approvals` choice, not an automatic conversion to skip-permissions. Its all-tool Hook is private CLI behavior, not a native stdin approval protocol. Scope verification must pass before starting the protected session; absence/failure never grants access.

The existing gate validates loopback token, confirmed parent/child conversation identity, active Turn and cancellation both before asking and after receiving the decision. Added regression covers an unregistered child and a registered child's pending allow arriving after the parent ends (both deny). The Windows executable environment reference is now quoted for installation paths containing spaces.

`ask_question`/`ask_permission` are currently denied by this bridge, not implemented as ordinary tool approvals or automatically answered. Native child Hook inheritance across every CLI version is not established by unit tests. Previous real approve/deny receipts remain evidence only for their tested versions/actions. No broader permissions were silently introduced.

## Background Claude children

Retain each explicitly announced child's originating event sender and task/tool aliases. Child text/tools and settlement go to that original Turn even if another Turn has started. Duplicate settlements are suppressed; unconfirmed children become interrupted when the native process exits. Parent terminal delivery no longer waits for child completion: Core continues only explicit child presentation events on a background consumer after persisting the parent terminal. An obsolete native Session cannot write into a replacement mapping. Background thought text is not persisted.

Storage integration test checks parent terminal preceding child settlement, original Turn attribution and identical projection after rebuild. Native task-created notifications must supply an explicit relationship; unknown relationships are not guessed.

## ACP / Cursor activity

Coalesce replacement content for tool calls. Publish bounded, complete native diffs only on confirmed success; failures and duplicate terminal updates do not publish additional diffs. Preserve absent/null old content versus an existing empty file, whitespace and final newline. Omit malformed Cursor creation pairs (`-- /dev/null`, `++ b/...`), oversized patches and computations exceeding the deadline. Workspace snapshots remain the authoritative Turn summary, including shell edits and operations ACP omits.

Cursor Tasks retain pending/running states and native prompt/result. Validate `cursor/task` against a known completed invocation before acknowledging it. A successful background launch is not child completion; unconfirmed work becomes interrupted when observation ends. Details explicitly state that native internal steps are unavailable. Native Cursor private store import/replay identity decoding has not been introduced; dogcodes replays its own persisted events.

## Session identity compare-and-swap

After native create/resume returns, verify active run ID, expected mapping ID, native Session ID and mapping checkpoint. Append only against the current task stream version; on conflict reread and revalidate. Apply the same checks to a late resume failure. A successful resume advances the mapping checkpoint. This prevents both stale success and stale failure from replacing or invalidating a newer Session.

## CodeBuddy (experimental)

Discover the installed `codebuddy --acp`; use existing native authentication. Register through the ACP readiness handshake. Model/thinking use native config options; mode selection uses `session/set_config_option` and requires native confirmation. Never fabricate modes. Existing native approvals, streaming, cancellation, workspace change captures and application history persistence are reused.

Use a fresh connection between Turns, including after cancellation, avoiding retained native cancellation state. Resume is advertised only when negotiated by the installed CLI. Live child traffic uses explicit `codebuddy.ai/parentToolCallId`; captured child activity is labeled partial. No private transcript-file watcher/import is claimed. Real acceptance on user-installed CLI 2.150.0 verified native models, approval/rejection, session resume and correlated live child events. User credentials were not modified.

CodeBuddy shell tools require the process working directory as well as the ACP session cwd. Restart its connection when the workspace changes, and launch it in that workspace. Discover NVM and NVM-arm installations without shell PATH and prepend the matching npm installation’s Node bin directory for desktop launches.

CodeBuddy SVG uses the purple rounded-square artwork supplied by the user; trademark ownership remains with its owner.

## Executed validation

- Antigravity bridge: 6 unit tests passed, including child registration/end-of-parent denial.
- ACP driver: 20 unit tests passed; 14 opt-in tests were not run by the suite.
- Claude driver: 17 unit tests passed before the additional idle-settlement fixture; real tests were not run in that suite.
- SQLite/Core: 38 tests passed, including background settlement after terminal/replay and existing resume-failure regressions.
- Native Cursor: `cursor_reports_confirmed_native_diff` passed against the installed CLI in a disposable workspace. File contents changed exactly as requested; exactly one successful native Diff was emitted. No real user project files were edited.
- Conversation rendering: 10 tests passed.
- CodeBuddy 2.150.0: native catalog returned 16 models. Real denial produced no file and an interrupted terminal; approval wrote the exact requested fixture in the isolated workspace. A fresh connection resumed prior context; a read-only delegated task produced 2 child lifecycle events and 19 correlated internal events. These counts describe streamed events, not 19 distinct tools or complete historical transcripts.
