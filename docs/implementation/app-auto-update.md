# Automatic app updates

dog.codes checks GitHub for a newer published release, downloads it quietly and offers to
install it. This covers the app itself; Agent CLI updates are separate
(`crates/agent-management/src/updates.rs`).

**Status:** the app side and the release flow are wired up. The public key is committed in
`plugins.updater.pubkey`, and `pnpm desktop:release` signs and publishes the update files
together with the installers. The first release that can update itself is the first one
built with this setup. People on 0.1.3 or earlier install that release by hand once.

## How it works

- **Source.** The official Tauri updater plugin (`tauri-plugin-updater`) reads
  `https://github.com/owlfy-ai/dogcodes/releases/latest/download/latest.json`
  (`plugins.updater.endpoints` in `apps/desktop/src-tauri/tauri.conf.json`; the repository
  comes from the `origin` remote). GitHub's `releases/latest` is the newest published release
  that is neither a draft nor a pre-release. The download host is not subject to the GitHub
  API rate limit.
- **Version.** An update must be a stable semver above the running version
  (`app_update::is_update`). Pre-release versions and build metadata never count as newer.
- **Platform.** The plugin picks this platform's entry (`darwin-aarch64`, `darwin-x86_64`,
  `windows-x86_64`, …) from `latest.json`. `scripts/updater-manifest.mjs` maps release asset
  names to these keys.
- **Verification.** Each package carries a minisign signature from the maintainer's updater
  key. With `requireSignedVersion: true`, the signature must also be bound to the announced
  version (`tauri signer sign --app-version`). This stops an old genuine package from being
  served as a newer one. The plugin verifies after downloading. The app verifies again before
  installing a package read back from disk.
- **Schedule.** The first check runs 90 seconds after launch, then every 6 hours. Offline,
  rate limiting, a missing `latest.json`, no package for this platform, and bad signatures are
  all logged to stderr and retried next round. None of them is shown to the person.
- **Download.** Happens in the background, into `<app cache>/app-update/<version>/package`.
  A verified package on disk is reused after a restart. Older versions are deleted.
- **Notice.** `AppUpdateNotice` is a small card in the bottom-right corner, not a dialog. It
  offers "Install and restart" and "Later". It is hidden while any local task has a running
  Agent turn. "Later" is stored per version (`localStorage` key `dogcodes.appUpdate.later`),
  so the same version is not offered again, but a newer one is.
- **Install.** The backend refuses with `busy` while the background service reports active
  work (the same check as closing the app). Otherwise it installs and restarts:
  - **macOS:** swaps the app bundle, then restarts.
  - **Windows:** stops the background service first, then runs the NSIS installer in passive
    mode, which restarts the app.

  Development builds only check when `DOGCODES_APP_UPDATE_ENDPOINT` points at a test server,
  and they never install.

## Release flow

Release work happens in `scripts/`. Details are in [release/README.md](../../release/README.md).

- **Key.** The updater key was created with `tauri signer generate` and has a password. It
  lives in the ignored `release/private/updater.key` on each signing machine, with an offline
  backup. The release-input guard rejects `*.key` files. If the key is lost, installed copies
  can never auto-update again, and every user has to reinstall by hand.
- **Settings.** In `release/.env.local` (blank keys in `release/.env.example`) set:
  - `TAURI_SIGNING_PRIVATE_KEY_PATH`, or `TAURI_SIGNING_PRIVATE_KEY` (exactly one)
  - `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`, which is required so the signer never prompts

  `pnpm desktop:package --signed` (including `--check`) fails before building when these are
  missing or the public key is empty.
- **Signing** (`scripts/updater-signing.mjs`). The CLI runs
  `tauri signer sign --app-version <version> <file>` with `@tauri-apps/cli` 2.12.4. The key
  and password reach only that process, through its environment. They never go to
  `tauri build`, argv or other tools. Each `.sig` is then verified in Node against the
  committed public key and must be bound to exactly this version.
- **macOS** (`scripts/signing-macos.mjs`). After the app is notarized, stapled and assessed,
  `dog.codes-<v>-macOS-AppleSilicon|Intel.app.tar.gz` is built from the stapled app with
  `COPYFILE_DISABLE=1 tar --no-mac-metadata`. The archive must contain only `dog.codes.app`.
  It is unpacked and checked again with `codesign --verify --deep --strict` and
  `stapler validate`, then signed. Both files are exported next to the DMG. Universal builds
  produce no update archive.
- **Windows** (`scripts/package-desktop.mjs`). After `tauri build` and renaming, the final
  Authenticode-signed `dog.codes-<v>-Windows-x64-Setup.exe` gets its `.sig`. People download
  the same file the updater runs.
- **Upload** (`scripts/desktop-release.mjs`, receipt schema 2). Each machine's receipt holds:
  - macOS: DMG, archive and `.sig` per architecture
  - Windows: installer and `.sig`

  Before uploading, signatures are verified locally. `refreshChecksums` then runs under the
  release lock: it adds the archives and signatures to `SHA256SUMS.txt`, refuses orphan
  signatures or a partial set of archives, writes `latest.json` from the uploaded `.sig`
  assets (tag-based URLs, version-bound signatures only), and uploads it. A release counts as
  ready only when `latest.json` lists `darwin-aarch64`, `darwin-x86_64` and
  `windows-x86_64`. The release notes show the in-app update status.
- **Publish.** Installed apps read `latest.json` of the latest published release, so
  publishing the draft rolls the update out. Apps pick it up 90 seconds after their next
  launch, or within 6 hours. A published release without `latest.json` is ignored silently.
- **Manual fallback.** `node scripts/updater-manifest.mjs owlfy-ai/dogcodes vX.Y.Z` rebuilds
  and uploads `latest.json` from the signatures already on a release.
- **Tauri config.** `app.macOSPrivateApi` is `true` because the computer-use notice windows
  are transparent. Tauri CLI 2.12 removes the `macos-private-api` crate feature unless this
  setting is present.

Linux: the plugin can update AppImage installs but not `.deb`. Linux packaging is paused, so
`latest.json` currently only lists macOS and Windows.

### Checking a real signed release

Run the signing preflight on each machine. Mac:

```sh
pnpm desktop:package --signed --check --target aarch64-apple-darwin
```

Windows:

```powershell
pnpm desktop:package --signed --check
```

Then run `pnpm desktop:release` on both machines. Before publishing the draft, confirm that
the release shows `latest.json`, two `.app.tar.gz` files with `.sig`, a `-Setup.exe.sig`,
and the release-notes row "In-app updates | Signed update packages…". Publish. The release itself
can only be updated *to* from a build that already contains the updater and this public key,
so the first full check runs one release later. On a Mac running the previous release,
launch dog.codes, wait about two minutes for the notice, choose "Install and restart", and
check the version the app reports.

## Later improvement

`tauri-plugin-updater` is pinned to `~2.12.0`, because 2.13 and later require the `tauri`
crate 2.12. After the app moves to Tauri 2.12, plugin 2.14's `Updater::restore_update` can
restore cached release metadata. That would let a downloaded update be offered offline right
after launch.

## Tests

- `apps/desktop/src-tauri/src/app_update/tests.rs`: version comparison, platform keys,
  signature and version checks, and the whole check → download → verify → announce flow
  against a mocked release server (including offline, rate-limited, missing-platform and
  bad-signature cases).
- `apps/desktop/src/AppUpdateNotice.test.tsx`: when the notice shows, "Later" per version,
  waiting during a running turn, and install errors.
- `scripts/updater-manifest.test.mjs`: choosing release assets per platform, refusing
  signatures without a version, and the upload.
- `scripts/updater-signing.test.mjs`: key handling (one source, password required, never in
  argv), signature verification against Tauri CLI fixtures, archive contents, and a real
  `tauri signer` run with a temporary password-protected key, including a wrong password.
- `scripts/desktop-release.test.mjs` and `scripts/package-desktop.test.mjs`: receipts,
  uploads, `SHA256SUMS.txt`, `latest.json`, readiness, the Mac archive step, and the blank
  `.env.example` keys.

The fixtures in `src-tauri/src/app_update/fixtures/` use a throwaway test key. Its private
half was discarded.
