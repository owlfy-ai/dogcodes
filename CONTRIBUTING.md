# Contributing to dog.codes

Thanks for helping. dog.codes is a development preview: bug reports with clear
reproduction steps are as valuable as code.

## Before you start

- Search existing issues first. For larger changes, open an issue to agree on the
  approach before writing code.
- Never include credentials, tokens, private repository content or personal data in
  issues, logs, screenshots or commits. Remove them from diagnostics exports before
  attaching them.
- Security problems go through the process in [SECURITY.md](SECURITY.md), not public issues.

## Development setup

Install Rust (the version in `rust-toolchain.toml`), Node.js 22+, pnpm (the version
in `package.json`) and the [Tauri platform prerequisites](https://v2.tauri.app/start/prerequisites/).

```bash
pnpm install --frozen-lockfile
pnpm desktop:dev
```

`pnpm dev` starts a browser-only UI demo with an in-memory fake client; it does not
run real agents, terminals or Git.

## Checks to run before a pull request

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --lib --tests --locked
pnpm check
pnpm test:release
pnpm check:release-inputs
```

CI selects checks from the changed paths on pull requests and pushes to `main` or
`master`. Documentation-only changes run the inexpensive CI routing, release script
and release-input checks. Frontend changes run `pnpm check`; native changes run
Clippy and native tests on Windows, macOS and Linux, with formatting checked once
on Linux. Frontend and native jobs run in parallel when both are needed. Remote
video/transport changes also run their Linux acceptance fixtures. Shared dependency
or CI changes run all checks; unavailable diff history falls back to all checks.
Path selection lives in `scripts/ci-changes.mjs`.

Ordinary pushes and pull requests do not build installers. Use **Run workflow** on
`Desktop checks and unsigned packages` for all checks, all acceptance fixtures and
unsigned installers on all four platform/architecture targets. Release tags reuse
that full workflow before creating a draft release. The `checks` job is the stable
aggregate status for branch protection, including documentation-only changes.

Tests that need a real agent account stay opt-in; describe any manual verification
you did in the pull request.

## Architecture in one paragraph

A background service (the "core service", started by the desktop app from the same
executable) owns agent sessions, approvals, reviews, Git and file operations,
terminals, project runs and shared settings. The desktop window is a client of that
service over an authenticated local connection: closing the window does not stop
work. Multi-step business flows (sending a message, switching agents, starting a
review repair) are driven by the service with fixed operation identities, so any
client can discover, continue or abandon them. New features should follow the same
rule: put business state and side effects in the service (`crates/core`,
`apps/desktop/src-tauri/src/daemon*`, `*_service.rs`) and keep the window to
presentation and OS-specific actions such as file pickers. This keeps the door open
for the planned mobile client, which will talk to the same service.

## Code and documentation conventions

- Match the style of the surrounding code; keep `unsafe` out of the workspace.
- Frontend UI conventions (loading states, tooltips, agent pickers) are in
  [`apps/desktop/src/AGENTS.md`](apps/desktop/src/AGENTS.md).
- Project status lives in [`docs/CURRENT_WORK.md`](docs/CURRENT_WORK.md); design notes and
  verification records are under `docs/implementation/`. Most internal notes are in
  Chinese; English contributions are welcome.
- Signing and notarization settings are local only; see [`release/README.md`](release/README.md).
  Do not commit filled-in signing files.

## License

By contributing, you agree that your contributions are licensed under the
[Apache License 2.0](LICENSE).
