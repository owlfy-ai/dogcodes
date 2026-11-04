# Windows agent scan diagnostics

The original local diagnostic package preserves process startup behavior so the
console window issue can be reproduced without changing its cause. The working
tree now also includes upstream fix `cf2b0eb`; that earlier package predates it.

## Local test

1. Exit the previous dog.codes instance, then install the local Windows x64 NSIS package.
2. Start dog.codes and allow the agent scan to finish. If a terminal appears,
   note the time and its executable title, and whether it closes by itself.
3. Open `%APPDATA%\codes.dog.desktop\diagnostics` and retain the newest
   `core-*.log` files (both desktop and Core write their own files).
4. The new build identifies itself with `diagnostics_config` containing
   `schema=process-scan-v1` and `build=release`.

## Recorded metadata

- `agent_scan`: provider, start/end/interruption, elapsed time and result.
- `agent_discovery_*`, `agent_version_probe_*`, `agent_readiness_*`: selected
  executable, protocol, version, status and safe failure classification.
- `process_spawn_attempt`: executable, working directory, argument count,
  recognized command words and launch strategy. Unrecognized arguments are omitted.
- `process_spawned`: child PID. The outer record's `pid` is its parent process;
  `launch_id` correlates records for the same launch within that parent.
- `process_scope_ready`: Windows job assignment and thread resumption completed.
- `process_failed`, `process_timeout`, `process_exited`: launch stage and OS
  error, direct-process timeout, exit code and elapsed time.
- `process_observer_dropped`: observation ended without a confirmed exit;
  this does not assert that the process survived cleanup.

Windows managed probes and driver processes record flags `0x08000004`
(`CREATE_NO_WINDOW | CREATE_SUSPENDED`). After upstream fix `cf2b0eb`, direct
bounded commands also hide their window and record `0x08000000`
(`CREATE_NO_WINDOW`). Logging itself does not change either strategy. Processes
independently created inside an external CLI are not directly instrumented.

The existing bounded, rotating service logger applies secret and home-path
redaction. Environment values, stdin, arbitrary arguments, stdout and stderr
are not recorded. Normal diagnostic-summary export remains a recovery-state
summary; use the log files above for this investigation.

## Build

```powershell
pnpm desktop:package --unsigned --target x86_64-pc-windows-msvc --bundles nsis
```

This builds locally and does not publish a GitHub release. Product version stays
at 0.1.1; the delivered copy uses a diagnostic filename to distinguish it.
