# Native protocol observability probes

These are opt-in historical maintenance tools, not desktop release acceptance tests.
They start real authenticated Agent processes and may consume quota. Recheck the
current Agent CLI protocol and use fresh isolated data before an authorized run.
Importing `probe.py` does not start an Agent.

Executable selection uses PATH by default: `codex`, `claude`, `cursor-agent`,
`kimi`, `grok`, and `agy`. Override a program with
`DOGCODES_PROBE_<PROVIDER>_EXECUTABLE`, where PROVIDER is `CODEX`, `CLAUDE`,
`CURSOR`, `KIMI`, `GROK`, or `ANTIGRAVITY`. The history-tool probe also accepts
`CODEBUDDY`. Values are one executable path, including paths with spaces; do not
include flags or shell commands. `~` is expanded to the current user's home.

ZCode still defaults to its macOS application bundle. Use
`DOGCODES_PROBE_ZCODE_EXECUTABLE` and `DOGCODES_PROBE_ZCODE_SERVER` to identify the
application executable and its `zcode.cjs` entry point. The history probe's runtime
helper uses the same executable. These overrides do not establish Windows/Linux
protocol compatibility.

Each invocation creates local files in `runs/`. Raw traces may contain task content
or native identifiers even after the probe's best-effort filtering. They are
excluded from public source under the [evidence policy](../../docs/implementation/source-evidence.md).
