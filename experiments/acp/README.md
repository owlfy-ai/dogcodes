# ACP stable-v1 M0 probe

This experiment pins the official Rust SDK with default features disabled. `Client.builder()` is
the stable-v1 entry point; draft v2 and MCP-over-ACP features are not compiled.

The in-memory fake Agent contract currently verifies initialize/capabilities, session/new,
session/prompt, session/update ordering, permission option preservation with a cancelled response,
and session/cancel. It never auto-approves a permission request.

The SDK's typed stable-v1 extension enum currently removes the leading `_` from custom methods.
`PreservedExtension` records the raw method and opaque parameters before typed dispatch; a future
Product Driver must keep this lossless interception point. The redacted evidence fixture is
`fixtures/stable-v1-offline-contract.json`.

No Product Driver is exposed from this crate. A real Agent executable must first pass initialize,
authentication-state, session/new, prompt, permission, cancellation, load-session, unknown-field,
and shutdown probes with redacted fixtures.

The current machine has the Cursor editor wrapper but not a separately installed Cursor Agent
runtime. Never use `cursor agent` as a background version probe: that wrapper attempts an install.
The generic `agent` executable on this machine identifies as Grok Build, not Cursor Agent.

Run the offline gate with:

```bash
cargo test -p acp-probe
cargo clippy -p acp-probe --all-targets -- -D warnings
```
