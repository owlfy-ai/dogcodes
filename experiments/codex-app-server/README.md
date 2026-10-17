# Codex App Server M0 probe

This binary is an experiment, not a Product Driver. It records version-specific schema facts and a redacted initialize handshake without starting a model turn.

```bash
cargo run -p codex-app-server-probe -- \
  schema /absolute/path/to/codex /tmp/codex-schema

cargo run -p codex-app-server-probe -- \
  handshake /absolute/path/to/codex

cargo run -p codex-app-server-probe -- \
  thread-start /absolute/path/to/codex

cargo run -p codex-app-server-probe -- \
  resume-smoke /absolute/path/to/codex

# Explicit opt-in required: this command calls a model and consumes Codex quota.
cargo run -p codex-app-server-probe -- \
  turn-smoke /absolute/path/to/codex --allow-model-usage

# Also calls a model; sends turn/interrupt immediately after turn/start.
cargo run -p codex-app-server-probe -- \
  interrupt-smoke /absolute/path/to/codex --allow-model-usage

cargo run -p codex-app-server-probe -- \
  fingerprint /absolute/path/to/codex_app_server_protocol.v2.schemas.json
```

Add `--experimental` to schema generation only when comparing the explicitly unstable surface.

Safety boundaries:

- executable and output paths must be absolute;
- schema output must be absent or empty and is never deleted by the probe;
- child processes use structured argv and bounded deadline/output; schema generation inherits only `PATH`, while handshake additionally passes the existing platform home/temp locator variables so Codex can manage its own runtime state;
- the probe itself never opens or prints those directories, credentials, or configuration;
- handshake output reports only the user-agent product prefix, platform fields, and whether `codexHome` was present; it never prints machine details or the home path;
- `thread-start` creates only an ephemeral thread with `/tmp`, `read-only`, and `approvalPolicy=never`, then probes immediate `thread/resume`; no thread id/path or JSON-RPC error message is printed;
- resume reports retain only the JSON-RPC error code and safe structural fields; the current immediate ephemeral probe is a negative test, not evidence about cross-process persistent resume;
- `resume-smoke` uses a fresh temporary `CODEX_HOME`, verifies the initialize response resolves to that exact canonical directory, applies a fixed `thread/name/set` persistence trigger, restarts app-server, and removes the temporary state on return;
- `turn-smoke` is impossible to invoke without the literal `--allow-model-usage` gate; it uses a fixed no-tools prompt and never prints response text or native ids;
- `interrupt-smoke` has the same explicit model-usage gate, issues a fixed no-tools prompt, immediately requests `turn/interrupt`, and retains only response flags, method/byte counts, and terminal status;
- turn output records only method counts, byte counts, terminal status, and observed capability labels.

The report includes both a raw SHA and a canonical SHA computed after recursively sorting JSON object keys. Compatibility gates use the canonical SHA because some Codex versions emit semantically identical objects in nondeterministic key order.

Committed facts belong in `fixtures/` and `docs/compatibility/codex.md`. Do not commit a user's raw stderr, paths, account fields, thread identifiers, prompts, or complete generated bundles.

## Offline sanitized diagnostic copies

These commands read explicitly supplied local files only. They do not start an Agent, read credentials,
install a runtime, or use model quota:

```bash
cargo run -p codex-app-server-probe -- \
  sanitize-fixture /absolute/path/to/private-frames.jsonl /absolute/path/to/new-safe-copy.jsonl

cargo run -p codex-app-server-probe -- \
  check-sanitized-fixture /absolute/path/to/new-safe-copy.jsonl
```

- Input must be an absolute regular-file path, not a final-component symlink. Export never modifies
  input or overwrites an existing output; all frames are validated before a private temporary file is
  written and atomically published without clobbering. Unix output is mode 0600.
- The code-owned schema/path policy drops unknown and sensitive keys, masks IDs/body/result/error
  text, and permits only reviewed method/status enums and bounded error codes. No free text is allowed
  by the current Codex policy. Live approval IDs are not changed.
- Import is limited to 1 MiB / 256 frames; each frame has a 64 KiB limit and depth/node/array/string
  budgets. Output is limited to 2 MiB. Any malformed or over-budget frame rejects the whole export.
- Each record carries policy, ruleset, parser/driver versions, `sourceKind: offline_import`, and
  `nativeVersion: null`. These are minimized diagnostic copies, not native JSON-RPC replay fixtures
  or proof that a particular Codex runtime emitted the frames.
- The read-only checker accepts only this version's canonical JSONL format, re-applies the policy,
  and rejects extra metadata, unsafe values, duplicate keys and noncanonical bytes. Source audit
  counts are range/shape checked, not authenticated. A nonzero exit is a failed check.
- Existing summary-report fixtures are a different format and remain unchanged. This boundary does
  not retrofit the older probe report paths. There is no production recorder, rotation/retention,
  compression, or repository-wide CI scan yet.

Implementation, budgets, synthetic CLI tests and handoff:
[`m1-s5-raw-diagnostics.md`](../../docs/implementation/m1-s5-raw-diagnostics.md).
