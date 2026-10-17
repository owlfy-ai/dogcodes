# Desktop packaging

The first release targets Windows, macOS and Linux. These build instructions do
not mean those platforms have passed release acceptance yet. See
[the current work list](../docs/CURRENT_WORK.md) for the release gates.

Install the [Tauri native prerequisites](https://v2.tauri.app/start/prerequisites/),
the Rust toolchain from `rust-toolchain.toml`, Node.js 22 and the pnpm version in
`package.json`. From the repository root:

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

## Local credentials, configured by the maintainer later

Copy `.env.example` to `.env.local` and `tauri.signing.example.json` to
`tauri.signing.local.json`, in this directory. All fields are intentionally empty.
The local files and `release/private/` are ignored by Git. Store private keys and
certificates in `release/private/` or outside the checkout; do not fill the
committed examples. Do not put credentials in shell command arguments or a
tracked workflow. Use secret environment variables in CI.

Run `pnpm desktop:package --signed` only after filling the required settings:

- **macOS:** set a Developer ID identity available in the keychain, or provide
  `APPLE_CERTIFICATE` and its password. Choose exactly one complete notarization
  method from `.env.local`: App Store Connect API or Apple ID with an app-specific
  password. An API key path must be absolute. Existing environment values take
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
nor accounts are created by the build. Tauri performs signing and notarization;
verify the resulting signatures and installed application before publication.
The script does not upload artifacts or create a GitHub Release.

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

Pushing a tag such as `v0.1.0` runs `.github/workflows/release.yml`. It first checks
that the tag matches the versions in `package.json`, `apps/desktop/package.json`,
`tauri.conf.json` and the Cargo workspace, then builds unsigned installers on
Windows (NSIS), macOS (arm64 and x64 DMG) and Linux (deb, AppImage), runs the checks
and tests on each platform, and creates a **draft pre-release** with all installers and
`SHA256SUMS.txt`. Review the draft, install from it, and publish it by hand. Signed
builds are not produced by CI until signing secrets are configured.

The `Desktop checks and unsigned packages` workflow checks the frontend and native
code, then builds Windows x64 NSIS, macOS ARM64/x64 DMG and Linux x64 DEB/AppImage
artifacts. It has read-only repository permissions, uses no signing secrets and
does not publish a release. Its presence is not proof of passing CI: inspect an
actual run, then test installation and daily workflows on each platform.

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

The first GitHub repository will start from the reviewed source snapshot. The
development repository and its existing history stay in their current location;
do not push that history to the new public repository.

Review any non-ignored untracked source files first. Put only their reviewed
repository-relative paths in a JSON array saved under the ignored
`release/private/` directory, then export to a **new** ignored directory:

```sh
pnpm source:snapshot --output release/private/github-source-candidate \
  --reviewed-untracked release/private/reviewed-source-files.json
```

The exporter includes working-tree edits and reviewed new source, preserves
deletions and executable permissions, and rejects private/generated paths,
symbolic links, unresolved index conflicts, filled signing examples, unreviewed
new files and changes during export. It checks that local signing inputs remain
ignored. It refuses to overwrite an existing candidate. No `.git`, dependency
directory, local configuration or prior development history is copied.

A sibling `.manifest.json` records each exported file's SHA-256 and size, the
reviewed new inputs and deletions. Keep it local for candidate review; it is not
copied into the public source tree. Exporting does not create a GitHub repository,
commit, push, tag, installer or Release. Validate the exported source separately
before choosing the initial public commit.

README screenshots are captured from existing product components using isolated
demo data. See [screenshot maintenance](../docs/README-MAINTENANCE.md); they do not
replace real-provider or native-platform acceptance.
