# History tool injection probe

This opt-in historical probe imports the native protocol helper from
[`subagent-observability`](../subagent-observability/README.md). The same executable
environment overrides apply, including `DOGCODES_PROBE_CODEBUDDY_EXECUTABLE`.

It creates isolated fixture files and starts real Agents. Running it requires
appropriate credentials and explicit intent to consume quota; it is not an
offline test or a release build dependency. Validate current CLI/protocol support
before reuse. `HISTORY_PROBE_RESUME_ROOT`, when explicitly set, must refer only to
the operator's prior isolated probe output, never production application data.

Generated JSON, traces and fixture text remain local and are excluded from source
release. See the [evidence policy](../../docs/implementation/source-evidence.md).
