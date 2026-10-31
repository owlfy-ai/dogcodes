# Desktop packaging

The first release targets Windows, macOS and Linux. These build instructions do
not mean those platforms have passed release acceptance yet. See
[the current work list](../docs/CURRENT_WORK.md) for the release gates.

The macOS package requires macOS 12.3 or later because the application links
ScreenCaptureKit for remote desktop video. `bundle.macOS.minimumSystemVersion`
in the shared Tauri configuration declares this floor for both architectures;
signing configuration overlays keep it. This dependency requirement does not
replace testing the installed application on the supported macOS versions.

Install the [Tauri native prerequisites](https://v2.tauri.app/start/prerequisites/),
the Rust toolchain from `rust-toolchain.toml`, Node.js 22 and the pnpm version in
`package.json`. From the repository root:

On Linux, install the native packages listed in
[the remote desktop prerequisites](../crates/remote-desktop/README.md), then
prepare SPA build headers before Cargo or packaging. Ubuntu 22.04's system
headers are older than the current Rust bindings. The helper uses a pinned
commit and verified archive, installs only headers in the ignored build cache,
and keeps the system PipeWire runtime:

```sh
spa_sdk_env="$(python3 scripts/prepare_linux_spa_sdk.py --print-env)" && eval "$spa_sdk_env"
```

The desktop and release workflows do this automatically. Systems with sufficient
headers use their installed SDK. `--archive /path/to/the/pinned/archive.tar.gz`
supports offline preparation with the same mandatory checksum verification.

The DEB declares PipeWire (`libpipewire-0.3-0 >= 0.3.48`) and GStreamer
core/base (`>= 1.20`) as required runtime libraries. PipeWire is linked by the
application even on X11; it must be installed before the executable can load.
GStreamer plugins and GPU drivers remain optional, with software encoding as
the fallback. A current ARM64 debug DEB has been built and installed in a clean
Debian 12 container without recommended packages: runtime libraries resolved,
and the desktop entry, icons and license resources matched the sources. See
[the validation record](../docs/implementation/linux-hardware-video-2026-10-26.md#2026-10-27deb-实际构建与干净环境安装).
Release builds on the target distributions and actual desktop/GPU operation
still require validation.

For AppImage builds, also install `file`, `patchelf`, `libpipewire-0.3-modules`
and `libspa-0.2-modules`. The packaging command resolves the native SDK paths
using `pkg-config` and includes SPA support, native protocol and client-node
libraries, plus a minimal client configuration. The AppImage launcher selects
these paths before executing the desktop binary; no PipeWire server is started.
This is needed because automatic ELF dependency scanning misses dynamically
loaded client modules. Use `pnpm desktop:package`, which supplies this AppImage
configuration, rather than invoking `tauri bundle` alone.

The current ARM64 debug AppImage passed relocation, library/resource checks on
a Debian 12 baseline with only the excluded base desktop libraries installed,
and the owned Portal/cursor/software-video fixture using its packaged client.
These checks do not establish release-mode, x64, real desktop/GPU or driver
compatibility. To repeat the packaged-client fixture on Linux after building:

```sh
python3 scripts/wayland_acceptance.py --format rgb-cursor --appimage /path/to/dog.codes.AppImage
```

```sh
pnpm install --frozen-lockfile
pnpm check
pnpm test:release
pnpm check:release-inputs
pnpm desktop:package --unsigned
```

Build on the destination OS. Tauri writes packages under `target/release/bundle`
(or `target/<target-triple>/release/bundle` when `--target` is supplied). Use
`--bundles app` for a macOS application bundle, `--bundles nsis` for a Windows
installer, or `--bundles deb,appimage` for Linux packages. Cross-compiling and
running the installed application are separate verification steps.

The default is **unsigned** distribution packaging. On macOS it uses an ad-hoc
signature, without notarization. This mode ignores the local signing files and
removes inherited signing credentials. It is suitable for local build validation;
it does not establish trust for public distribution.

## Which release files belong in Git

This directory contains the public packaging instructions and two blank signing
examples. Git ignores its top-level contents by default, with explicit exceptions
only for `README.md`, `.env.example` and `tauri.signing.example.json`. Local
exports, installer outputs and private material placed here stay ignored.

Elsewhere, local Agent worktrees/launcher settings, package and tool caches,
Tauri `gen/schemas/`, root `artifacts/` acceptance captures, test reports,
databases and editor backups are ignored. Shared Agent commands/skills,
`capabilities/`, icons, source fixtures and reviewed documentation images remain
eligible for publication. The release-input guard also rejects these local paths
if they are force-added to the index; ignoring a path alone never untracks it.

## Local signing configuration

Copy `.env.example` to `.env.local` and `tauri.signing.example.json` to
`tauri.signing.local.json`, in this directory. All fields are intentionally empty.
The local files and `release/private/` are ignored by Git. Store private keys and
certificates in `release/private/` or outside the checkout; do not fill the
committed examples. Do not put credentials in shell command arguments or a
tracked workflow. Use secret environment variables in CI.

Run `pnpm desktop:package --signed` only after filling the required settings:

- **macOS:** for local signing, set `APPLE_SIGNING_IDENTITY` to the full
  `Developer ID Application: Name (TEAMID)` available in Keychain and
  `APPLE_NOTARY_PROFILE` to an existing `notarytool` Keychain profile. Set
  `bundle.macOS.hardenedRuntime` to `true` in the local JSON (the Keychain path
  also enforces this). Reuse a profile from another app when it belongs to the
  same signing team; no private-key export or plaintext password is needed.
  Alternatively, use `APPLE_CERTIFICATE` and its password, with exactly one
  complete notarization method: App Store Connect API or Apple ID with an
  app-specific password. Do not combine those credentials with a Keychain
  profile. An API key path must be absolute. Existing environment values take
  precedence over local values. See [Tauri macOS signing](https://v2.tauri.app/distribute/sign/macos/).
- **Windows:** fill the local JSON with the certificate thumbprint, SHA-256 digest
  and the issuer's timestamp URL, or configure `signCommand` for the chosen
  certificate provider. Provision the certificate/private key using that
  provider's supported hardware, certificate store or signing service. This
  script does not import PFX files or assume a certificate product. Avoid placing
  passwords in `signCommand`: the underlying signing tool may log its invocation.
  See [Tauri Windows signing](https://v2.tauri.app/distribute/sign/windows/).
- **Linux:** this script builds unsigned packages; repository/package signing can
  be configured separately when the distribution channel is chosen.

The local JSON accepts only signing-related fields: macOS identity, hardened
runtime, provider short name and entitlements; Windows certificate thumbprint,
digest, timestamp/TSP and signing command. Installer hooks, packaged files,
frameworks and other app/build settings belong in reviewed public configuration,
not in this private override. A Windows custom command may use Tauri's object
form (`cmd` and `args`) when paths contain spaces.

The signed mode fails early if required settings are empty. Neither certificates
nor accounts are created by the build. The scripts do not upload release assets
or create a GitHub Release. Mac notarization submits the app and DMG to Apple.

### Local Mac with Keychain credentials

```sh
# Check identity, profile access, Xcode tools and target; no build or submission.
pnpm desktop:package --signed --check
# Check both Rust targets together.
pnpm desktop:package --signed --check --target universal-apple-darwin
# Build each architecture on the Mac signing machine.
pnpm desktop:package --signed --target aarch64-apple-darwin --bundles dmg
pnpm desktop:package --signed --target x86_64-apple-darwin --bundles dmg
```

If the profile is missing on a new Mac, import the certificate **and private key**
through Keychain Access, then run `xcrun notarytool store-credentials PROFILE_NAME
--team-id TEAMID` interactively in your terminal. Enter the Apple ID and
app-specific password only at Apple's prompts. The packaging script does not
read or export the saved password. Preflight verifies profile access, not the
acceptance of a future submission.

With `APPLE_NOTARY_PROFILE`, packaging builds a signed Hardened Runtime app,
checks the CPU architecture, bundle ID and signing team, notarizes the app ZIP,
staples and assesses the app, then creates, signs, notarizes, staples and assesses
the DMG. Each submission must return `Accepted`. Only after every step succeeds
is the DMG and its SHA-256 file copied to
`release/installers/<version>/<target>/`. Explicit `--bundles app` exports only
the verified app there. Intermediate files under `target/` are not release
artifacts. Without an explicit target, this path builds the host architecture.
The API/Apple ID mode continues to use Tauri's own notarization workflow.

### Windows with SimplySign

On the Windows signing machine, copy the **local** configuration via a private
channel or fill its `bundle.windows` fields: `certificateThumbprint`,
`digestAlgorithm: "sha256"`, `timestampUrl: "http://time.certum.pl/"` and
`tsp: true` for Certum RFC 3161 timestamps. Use the actual current certificate
fingerprint from SimplySign; renewal may change it. Do not copy the reference
app's resource mappings, app identifier or installer hooks.

Log in to SimplySign Desktop under the same Windows user that builds the app.
The certificate must be available in `CurrentUser\\My` with access to its
private-key provider. Install Windows SDK SignTool and the normal build tools.

```powershell
pnpm desktop:package --signed --check
pnpm desktop:package --signed --target x86_64-pc-windows-msvc --bundles nsis
```

The certificate-store path checks certificate validity, code-signing usage,
private-key association and SignTool availability before building. It does not
prove that SimplySign will authorize a signature; login or a PIN may still be
required. Tauri signs the app and installer via SignTool. Verify the final
installer and the installed app signatures and timestamps on Windows before
publication. Output stays under `target/<target>/release/bundle/nsis/`.
`--check` does not support custom `signCommand` providers.

## Data compatibility between versions

User data lives in the app data directory (`app-v5.db` plus service files) and is
never deleted by an upgrade. The database records its format in `PRAGMA user_version`
(`STORE_FORMAT_VERSION` in `crates/store`, currently 1). Rules for every release:

- A change to the on-disk format must ship with a migration and a format bump in the
  same release; note it in the release notes.
- An older build that finds a newer format refuses to open it read-only and shows a
  dedicated startup message; it never reads or rewrites the file. Reinstalling the
  newer version restores access.
- A damaged database is also left untouched and reported, never recreated.

## Draft releases from CI

### One command on each signing machine

After the one-time setup below, run the same command from a clean checkout of
the **same pushed source commit** on Mac and Windows:

```sh
pnpm desktop:release
```

Mac builds both Apple Silicon and Intel DMGs, signs and notarizes them. Windows
builds and signs the x64 NSIS installer, extracts it with 7-Zip and verifies the
installer and packaged `desktop.exe` signatures and timestamps. The command then:

1. Creates and pushes `v<package.json version>` after successful local packaging,
   or reuses that tag if it already points to the same source commit.
2. Waits for the tag-triggered GitHub workflow to finish and create a draft with
   all five installers, including the Linux DEB and AppImage.
3. Uploads this machine's signed installers, replaces their unsigned CI variants,
   and leaves the other platforms intact. The two machines can run in either order.
4. Validates signed-package receipts against the actual uploaded files, updates
   `SHA256SUMS.txt` for **all five installers**, and updates the managed signing
   status section in the draft notes while preserving your other release notes.

Hashing uses the SHA-256 digest GitHub computes for uploaded assets; if it is
unavailable, the command downloads and hashes the actual file. It does not reuse
the checksum of an unsigned package. Public `signing-macos.json` and
`signing-windows.json` receipts record source commit, package names, sizes and
hashes; local paths and signing credentials are excluded.

The command keeps the Release as a **draft**. Wait for both machines to finish,
review installation results and the draft, then manually publish it (turn off
the pre-release option for a stable release). Do not publish while an upload
command is still running. This command does not replace already-published releases,
move existing version tags, automatically commit code or push source branches.

One-time prerequisites:

- Commit and push the release scripts and intended product changes first. The
  command rejects dirty worktrees, unpublished source commits and mismatched
  version fields; it does not silently include local edits.
- Install Node.js 22, pnpm, the pinned Rust toolchain and platform-native build
  prerequisites. The command installs the required Rust targets and JS
  dependencies with the frozen lockfile, and runs the release-script tests and
  release-input guard. CI runs the full checks and native tests.
- Configure the ignored local signing files described above. On Mac this command
  requires a Keychain notarization profile. On Windows it requires a certificate
  thumbprint and a logged-in SimplySign session; PIN prompts can still occur.
- Install full 7-Zip on Windows (or set `SEVEN_ZIP_PATH` to `7z.exe`), plus
  Windows SDK SignTool. The reduced `7za.exe` does not extract NSIS installers.
- Install GitHub CLI on both machines and run `gh auth login` once with access
  to the origin repository, including releases and Actions. Configure Git push
  authentication too (`gh auth setup-git` for HTTPS, or your existing SSH setup).

```sh
# Read-only checks; no packaging, tag creation or upload.
pnpm desktop:release --check
# Retry tag push / CI wait / upload without rebuilding or re-notarizing.
pnpm desktop:release --upload-only
```

Resume requires the same source commit and unchanged verified local packages.
Completed build receipts live under the ignored
`release/private/release-state/<tag>/`. If CI fails, inspect and rerun the failed
workflow first, then use `--upload-only`. If changing source is necessary after
the tag was pushed, bump all four version fields and release a new tag.

Draft updates use an exclusive `RELEASE_UPLOAD_LOCK.json` asset, acquired without
overwriting an existing lock, so simultaneous Mac/Windows uploads cannot race
their checksums. Normal failures release it. If a process crashes or loses access
before cleanup, stop that process and verify no other release command is running
before manually deleting the lock attachment in the draft, then resume. Locks
are never automatically stolen based on time alone.

### Tag-triggered workflow

Pushing a tag such as `v0.1.0` runs `.github/workflows/release.yml`. It first checks
that the tag matches the versions in `package.json`, `apps/desktop/package.json`,
`tauri.conf.json` and the Cargo workspace, then builds unsigned installers on
Windows (NSIS), macOS (arm64 and x64 DMG) and Linux (deb, AppImage), reuses the full
desktop checks and Linux acceptance fixtures, and creates a **draft pre-release** with all installers and
`SHA256SUMS.txt`. Review the draft, install from it, and publish it by hand. Signed
builds require a separate workflow change: the current workflow explicitly uses
`--unsigned` and cannot access local Keychain profiles or a local SimplySign
session. Filling local settings or adding Secrets alone does not enable CI
signing. If replacing draft assets with locally signed packages, regenerate
`SHA256SUMS.txt` from the final assets before publication.

On normal pushes and pull requests, `Desktop checks and unsigned packages` selects
frontend, native and Linux acceptance checks by changed paths and skips installer
builds. Documentation-only changes skip compilation. Select **Run workflow** to run
the full suite and build Windows x64 NSIS, macOS ARM64/x64 DMG and Linux x64
DEB/AppImage artifacts. The tag-triggered release workflow calls this same full
suite, including the optimized native PipeWire fixture for the release AppImage.
The desktop workflow has read-only repository permissions, uses no signing secrets
and does not publish a release. Its presence is not proof of passing CI: inspect
an actual run, then test installation and daily workflows on each platform.

`.gitignore` does not remove secrets already committed to history. Source-release
review must check the Git index and history separately before publishing.
`pnpm check:release-inputs` catches tracked local credential paths (including
force-added ignored files and uppercase certificate extensions), generated
dependency/build paths and filled signing examples. It checks both working files
and staged example bytes: restoring a blank working copy does not hide a filled
copy already in the index. It also rejects local experiment captures and everything
under `output/`, even if force-added. Experiment source and four explicitly
reviewed protocol JSON fixtures remain public; see the
[source evidence policy](../docs/implementation/source-evidence.md). The output says how many examples are actually staged;
untracked examples passing this check do not prove a complete release commit.
Parse/validation errors do not echo private JSON contents. The examples accept
only their documented blank fields and the public SHA-256 digest setting.

This is a configuration guard, not a complete secret, personal-data or license
audit. In particular, captured agent traces, diagnostics and screenshots need
separate review, including versions still reachable in Git history.

## GitHub source snapshot

The initial reviewed snapshot is already public on GitHub. The private development
history remains separate. The one-time snapshot exporter and its tests were
retired after that initial publication; subsequent releases use committed source
and version tags in the existing GitHub repository. See the
[initial publication record](../docs/implementation/github-source-release-checklist-2026-10-17.md)
for historical evidence.

README screenshots are captured from existing product components using isolated
demo data. See [screenshot maintenance](../docs/README-MAINTENANCE.md); they do not
replace real-provider or native-platform acceptance.
