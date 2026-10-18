# Security policy

## Reporting a vulnerability

Please do **not** open a public issue for security problems. Use GitHub's private
vulnerability reporting on this repository (**Security → Report a vulnerability**).
Include the affected version or commit, your platform, reproduction steps and the
impact you observed. We will acknowledge the report, keep you updated and credit you
in the fix notes if you wish.

## Supported versions

dog.codes is a development preview. Security fixes land on the default branch and in
the next preview release; older previews are not patched.

## Security model in brief

- dog.codes runs coding agents you have installed, with your user account's
  permissions, in the project folders you add. Agents can read and change files and
  run commands according to their own approval settings; review what you approve.
- The desktop window talks to a background service over a loopback-only connection
  authenticated with a random per-run token stored in a user-private file. The
  service is not meant to be reachable from other machines.
- Agent credentials stay with each agent's own tools; dog.codes does not collect or
  upload them. Diagnostics exports are redacted and size-limited, but check them
  before sharing.
- Preview builds may be unsigned; verify that downloads come from this repository's
  releases and match the published checksums.
