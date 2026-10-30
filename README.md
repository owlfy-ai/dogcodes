<!-- English feature illustrations live in docs/images/readme/en/. Other language artwork awaits review. -->
<div align="center">

<img src="apps/desktop/src-tauri/icons/128x128@2x.png" alt="dog.codes" width="96" height="96" />

# dog.codes

**Every coding agent. One workspace. Command it from anywhere.**

<strong>English</strong> · <a href="docs/i18n/README.zh-CN.md">简体中文</a> · <a href="docs/i18n/README.zh-TW.md">繁體中文</a> · <a href="docs/i18n/README.ja.md">日本語</a> · <a href="docs/i18n/README.ko.md">한국어</a> · <a href="docs/i18n/README.es.md">Español</a> · <a href="docs/i18n/README.fr.md">Français</a> · <a href="docs/i18n/README.de.md">Deutsch</a> · <a href="docs/i18n/README.pt-BR.md">Português (Brasil)</a>

</div>

dog.codes brings your coding agents, conversations, code changes, and project commands into one desktop app. Let one agent implement a feature, ask another to review it, then run the project and prepare a commit—all from the same workspace. When you step away from your desk, command it all from your phone.

[Website](https://dog.codes) · [Features](#features) · [Phone remote](#phone) · [Get started](#getting-started)

## Supported agents

Connect the agents you already use. The 12 integrations below are implemented; available models, approvals, usage data, and recovery behavior vary by agent and installed version. Install and sign in to your chosen agent through its own tools first.

<p align="center">
  <a href="https://developers.openai.com/codex/"><img src="docs/images/readme/agents/codex.png" alt="Codex" height="36" /></a>
  <a href="https://code.claude.com/docs/en/overview"><img src="docs/images/readme/agents/claude-code.png" alt="Claude Code" height="36" /></a>
  <a href="https://cursor.com/"><img src="docs/images/readme/agents/cursor.png" alt="Cursor" height="36" /></a>
  <a href="https://www.kimi.com/"><img src="docs/images/readme/agents/kimi.png" alt="Kimi Code" height="36" /></a>
  <a href="https://grok.com/"><img src="docs/images/readme/agents/grok.png" alt="Grok Build" height="36" /></a>
  <a href="https://z.ai/"><img src="docs/images/readme/agents/zcode.png" alt="ZCode" height="36" /></a>
  <br />
  <a href="https://antigravity.google/"><img src="docs/images/readme/agents/antigravity.png" alt="Antigravity" height="36" /></a>
  <a href="https://www.codebuddy.ai/"><img src="docs/images/readme/agents/codebuddy.png" alt="CodeBuddy" height="36" /></a>
  <a href="https://pi.dev/"><img src="docs/images/readme/agents/pi.png" alt="Pi" height="36" /></a>
  <a href="https://opencode.ai/"><img src="docs/images/readme/agents/opencode.png" alt="OpenCode" height="36" /></a>
  <a href="https://github.com/deepseek-ai/deepseek-harness"><img src="docs/images/readme/agents/deepseek-harness.png" alt="DeepSeek Harness" height="36" /></a>
  <a href="https://kiro.dev/"><img src="docs/images/readme/agents/kiro.png" alt="Kiro" height="36" /></a>
</p>

## One place to work

![One place to work — desktop first, phone where available](docs/images/readme/en/overview.png)

<a id="features"></a>

## Features

### Hand off work between agents seamlessly

**Switch the agent running the same conversation at any time; context is handed over automatically.**

- Give each task to the agent best suited for it — one designs the architecture, another fills in the details.
- When one agent runs out of quota, another picks up the work without you restating the requirements.
- The previous conversation travels with the handoff, and the new agent can look up earlier turns through built-in history tools — it truly continues instead of starting over.

![Hand off work between agents seamlessly — desktop first, phone where available](docs/images/readme/en/switch-agent.png)

### Know your quota and usage at a glance

**Track quotas, usage, reset times, and versions for all 12 supported coding agents in one place.**

- See every agent's remaining quota and reset time, plan your work around them, and make every token count.
- Manage agent versions in one place and upgrade with a single click.
- Installed agents are discovered automatically, along with their sign-in status.

![Know your quota and usage at a glance — desktop first, phone where available](docs/images/readme/en/usage.png)

### Use the subscriptions you already have

**dog.codes drives the official CLIs you have installed and signed in to — Claude Code, Codex, Cursor, and more — using your own plans.**

- No separate dog.codes account or extra API purchase: the plans you already pay for keep working.
- Local-first: your code, conversations and credentials stay on your computer. The optional phone link is end-to-end encrypted, so the relay never sees your data.
- Agents are launched with only the environment they need, so your other secrets are not exposed to them.

![Use the subscriptions you already have — desktop first, phone where available](docs/images/readme/en/accounts.png)

### Cross-agent code review and repair

**Ask a different agent to review the code that was just changed, catching bugs and design flaws, or to run a security or performance analysis.**

- Different agents and models review each other's work from a fresh context, avoiding the blind spots of a single agent in one conversation.
- Findings are ranked high, medium, and low; select the ones you want and start the repair with one click.
- The repair goes back to the main agent together with the review report and selected findings, keeping the context intact.

![Cross-agent code review and repair — desktop first, phone where available](docs/images/readme/en/review-fix.png)

### Fork across agents and compare approaches

**Fork a new conversation from any turn and let another agent try a different idea, leaving the original untouched.**

- With worktrees, the code changes of each approach stay isolated from one another.
- Let different agents each produce a version of the same task, then keep the best one.

![Fork across agents and compare approaches — desktop first, phone where available](docs/images/readme/en/fork.png)

### Git management that feels familiar

**Git operations modeled closely on VS Code, so you manage the Diff in the sidebar the way you already do.**

- Stage, unstage, discard, ignore, Commit, and Commit & Push, with tree and list views.
- Have any agent write the commit message with one click, in the language you choose.

![Git management that feels familiar — desktop first, phone where available](docs/images/readme/en/git-changes.png)

### Discover how to run a project, then run it in one click

**dog.codes analyzes how a project is run and debugged and turns those commands and scripts into clickable launch entries.**

- No more remembering each project's start command or opening a terminal just to launch it — one click runs it.
- Output is collected in one place so you can check it any time.

![Discover how to run a project — desktop first, phone where available](docs/images/readme/en/run-project.png)

### Multi-folder projects

**Frontend, backend, and docs in separate folders? Hand them all to the agent in a single conversation.**

- Even agents without native multi-folder support can work on multi-folder projects through dog.codes.

![Multi-folder projects — desktop first, phone where available](docs/images/readme/en/multi-folder.png)

### Per-turn usage, crystal clear

**See the token usage, context occupancy, cache hits, and cache usage of every turn.**

- Understand exactly what the current conversation and each turn consumed, and where your tokens went.

![Per-turn usage, crystal clear — desktop first, phone where available](docs/images/readme/en/turn-usage.png)

**Open source · Free · Cross-platform**: licensed under Apache-2.0, built for macOS, Windows, and Linux.

<a id="phone"></a>

## Command your agents from your phone

**Leave the laptop at home. Your agents keep working on your computer while your phone keeps command.**

Pair a phone once and run everything below from anywhere. Your code and your agents stay on your computer.

![Command your agents from your phone — desktop first, phone where available](docs/images/readme/en/phone-remote.png)

- **Continue or start conversations.** Pick up any conversation from your computer, or start a new one — choose the agent, model and thinking effort, watch the output stream in live, and search, pin, archive or fork sessions.

  ![Hand off work between agents seamlessly — desktop first, phone where available](docs/images/readme/en/switch-agent.png)

- **Approve and answer.** Permission requests and agent questions reach your phone with push notifications; one tap and the agent carries on. Every approval is checked against the original run on your computer.

  ![Approve permissions and answer questions — desktop first, phone where available](docs/images/readme/en/permissions.png)

- **Review and commit.** See the changes in every project folder, stage files, have an agent write the commit message, then commit, push, pull or sync. Browse commit history and compare branches read-only.

  ![Git management that feels familiar — desktop first, phone where available](docs/images/readme/en/git-changes.png)

- **Add projects from your PC.** Browse the folders on your computer and add one as a project without sitting back down at your desk.

  ![Add projects from your computer — desktop first, phone where available](docs/images/readme/en/add-project.png)

- **Run commands in your PC’s terminal.** Open terminals in any project folder and work in the real shell of your computer.

  ![Run commands in your PC terminal — desktop first, phone where available](docs/images/readme/en/terminal.png)

- **Run the project, see it on your phone.** Start the launch entries on your computer and open the running web app right on the phone.

  ![Discover how to run a project — desktop first, phone where available](docs/images/readme/en/run-project.png)

- **Browse files and send attachments.** Read project files, and attach files and images to your messages.

  ![Browse files and send attachments — desktop first, phone where available](docs/images/readme/en/files-attachments.png)

- **Remote desktop when you need it.** View your computer’s screen and tap, drag, scroll, pinch, type, or paste text and images — with quality that adapts to the network.

![Remote desktop when you need it — desktop first, phone where available](docs/images/readme/en/remote-desktop.png)

### How it connects

- Scan a QR code once; no account is needed. The link is end-to-end encrypted. Pair several computers, see which are online, and revoke a phone at any time from the desktop app.
- The phone connects directly when it can and falls back to the dog.codes relay service, which only forwards encrypted traffic.
- Your computer stays on with dog.codes open or minimized. While work or a remote session is active, dog.codes can keep the computer from sleeping automatically; you can turn this off.
- Remote desktop works with macOS, Windows and Linux (X11 and Wayland) computers; your operating system asks for the screen and input permissions it requires.

<a id="getting-started"></a>

## Get started

The macOS app requires **macOS 12.3 or later** for ScreenCaptureKit.

For now, run from source. Install **Rust 1.97.1**, **Node.js 22+**, **pnpm 11**, and the [Tauri platform prerequisites](https://v2.tauri.app/start/prerequisites/). Clone or download this repository, then run these commands from its root:

```bash
pnpm install
pnpm desktop:dev
```

1. Install and authenticate at least one supported agent using its official setup.
2. Open dog.codes, connect an available agent, and add your project folders.
3. Start a conversation, choose an agent and model, and describe the work.
4. Open the Run panel to discover project commands, or inspect changes and request a review.
5. To command your agents from a phone, open **Connect phone** in the desktop app and scan the pairing code with the phone app.

Agent conversations, project analysis, reviews, and commit-message generation use the selected agent’s account and may consume its quota.

<details>
<summary>For contributors</summary>

```bash
cargo test --workspace
pnpm check
```

`pnpm dev` opens a browser-only UI demo with a fake adapter. Use `pnpm desktop:dev` for real agents and project commands.

On macOS, `pnpm desktop:dev` uses `APPLE_SIGNING_IDENTITY` from the environment or Git-ignored `release/.env.local`, falling back to `bundle.macOS.signingIdentity` in `release/tauri.signing.local.json`. Use an installed certificate's full identity, such as `Developer ID Application: Name (TEAMID)`. Each build and Rust hot reload is signed before launch with the same identity and the DogCodes application identifier, preserving Keychain authorization across rebuilds. The first launch after switching from ad-hoc signing may need **Always Allow** once for the remote identity. Without a configured certificate, development uses the default ad-hoc signature. Run `pnpm test:desktop-dev` to check the launcher; set `DOGCODES_TEST_SIGNING_IDENTITY` to an installed identity to include the isolated macOS signing integration test.

Core is a child process owned by the desktop, started from the same installed build. **Restart background now** in the bottom-left preferences menu restarts it after confirmation.

</details>

## Project details

Read [CONTRIBUTING.md](CONTRIBUTING.md) before opening a pull request, and report security issues privately as described in [SECURITY.md](SECURITY.md).

Agent icons reuse the assets already included in this repository; see [icon provenance and notices](apps/desktop/src/assets/licenses/README.md). Product names and marks belong to their respective owners.
