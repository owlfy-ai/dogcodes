# Automatic app updates

dog.codes checks GitHub for a newer published release, downloads it quietly and offers to
install it. This covers the app itself; Agent CLI updates are separate
(`crates/agent-management/src/updates.rs`).

**Status:** the app side is built and tested. It stays switched off until the maintainer
creates an updater signing key and the release flow publishes signed updater files (see
[Maintainer setup](#maintainer-setup)). Until then the app logs
`app updates are off: tauri.conf.json has no updater public key` and does nothing else.

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

## Maintainer setup

The first release that can update itself is the first one that ships this code with a public
key set. People on 0.1.3 or earlier have to install that release by hand once.

1. **Create the updater key once**, on a trusted machine, with a strong password:

   ```sh
   pnpm --filter @dogcodes/desktop tauri signer generate -w release/private/updater.key
   ```

   `release/private/` is ignored by Git. `check:release-inputs` also rejects `*.key`. Back up
   the key and its password somewhere safe and offline. **If the key is lost, installed copies
   can never auto-update again**, and every user has to reinstall by hand.

2. **Commit the public key.** Copy the contents of `release/private/updater.key.pub` into
   `plugins.updater.pubkey` in `apps/desktop/src-tauri/tauri.conf.json`.

3. **Give both signing machines the private key.** Set these in each machine's ignored
   `release/.env.local` or in the shell, never on the command line:
   - `TAURI_SIGNING_PRIVATE_KEY_PATH` (or `TAURI_SIGNING_PRIVATE_KEY`)
   - `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`

   `release/.env.example` would need these two blank keys, and the `blankEnvironment` guard
   in `scripts/check-release-inputs.mjs` would need to allow them.

4. **Use Tauri CLI 2.12.4 or later for signing.** The `--app-version` flag of
   `tauri signer sign` first appeared in that line. The repository currently pins
   `@tauri-apps/cli` 2.11.4, whose signatures carry no version. The app rejects those, and
   `updater-manifest.mjs` refuses them at release time. Raise the devDependency in
   `apps/desktop/package.json` to `^2.12.4`, then confirm with
   `pnpm desktop:package --unsigned`. The Rust `tauri` crate can stay on 2.11.

5. **Sign the final packages** with
   `tauri signer sign --app-version <version> <file>`, which writes `<file>.sig`. Sign the
   finished files, not `bundle.createUpdaterArtifacts`, because the files that get published
   are the ones changed after `tauri build`:
   - **macOS** (`scripts/signing-macos.mjs`, after `stapler validate` and `spctl --assess` of
     the app): build the archive from the stapled app with
     `COPYFILE_DISABLE=1 tar -czf <name> -C <dir> dog.codes.app`. Get `<name>` from
     `macUpdaterArchiveName(productName, version, target)`, for example
     `dog.codes-0.1.4-macOS-AppleSilicon.app.tar.gz`. Sign it, and add both files to the
     exported files and the signed-package receipt next to the DMG.
   - **Windows:** after `verify-windows-release.ps1` passes, sign
     `dog.codes-<v>-Windows-x64-Setup.exe` (the same installer people download). Add the
     `.sig` to the receipt.

6. **Upload them and `latest.json`.** Proposed changes to `scripts/desktop-release.mjs`:
   - `installer()` and the receipt checks should also accept `.app.tar.gz`, so the archive is
     uploaded and listed in `SHA256SUMS.txt`. The `.sig` files are uploaded with their
     packages.
   - In `refreshChecksums`, still holding the release lock, call `updaterManifest()` with the
     release's asset names and `.sig` contents. Upload the result as `latest.json`.
     Regenerating it on each machine's run is safe: the draft is private until it is
     published.

   Until that is wired in, run this once both machines have uploaded, and before publishing
   the draft:

   ```sh
   node scripts/updater-manifest.mjs owlfy-ai/dogcodes v0.1.4
   ```

   The command downloads the `.sig` assets, checks that each is bound to the version, writes
   tag-based URLs (draft assets live under a temporary `untagged-…` path), and uploads
   `latest.json`.

7. **Publish the draft.** Running apps pick up the update within 6 hours, or 90 seconds after
   their next launch. A published release without `latest.json` is ignored silently.

Linux: the plugin can update AppImage installs but not `.deb`. Linux packaging is paused, so
`latest.json` currently only lists macOS and Windows.

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

The fixtures in `src-tauri/src/app_update/fixtures/` use a throwaway test key. Its private
half was discarded.
