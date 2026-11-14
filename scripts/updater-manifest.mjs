import { execFileSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// The in-app updater (apps/desktop/src-tauri/src/app_update.rs) reads `latest.json` from the
// newest published Release. Each entry pairs a package with its minisign signature (`<name>.sig`,
// made with `tauri signer sign --app-version`). Package names follow scripts/release-names.mjs.
export const updaterPackages = {
  'macOS-AppleSilicon.app.tar.gz': 'darwin-aarch64',
  'macOS-Intel.app.tar.gz': 'darwin-x86_64',
  'Windows-x64-Setup.exe': 'windows-x86_64',
  'Windows-arm64-Setup.exe': 'windows-aarch64',
  'Linux-x64.AppImage': 'linux-x86_64',
  'Linux-arm64.AppImage': 'linux-aarch64',
};
export const manifestName = 'latest.json';

/** The name of the signed app archive for a macOS Rust target. */
export function macUpdaterArchiveName(productName, version, target) {
  const label = { 'aarch64-apple-darwin': 'macOS-AppleSilicon', 'x86_64-apple-darwin': 'macOS-Intel' }[target];
  if (!label) throw new Error(`No updater package for ${target}.`);
  return `${productName}-${version}-${label}.app.tar.gz`;
}

/** The version bound into a Tauri updater signature, from its signed trusted comment. */
export function signedVersion(signature) {
  const text = Buffer.from(signature.trim(), 'base64').toString('utf8');
  const comment = text.split('\n').find(line => line.startsWith('trusted comment: '));
  return comment?.slice('trusted comment: '.length).split('\t').find(field => field.startsWith('version:'))?.slice('version:'.length) ?? null;
}

/**
 * Builds the manifest from a release's asset names and the text of their `.sig` files.
 * URLs use the final tag path, since draft assets live under a temporary `untagged-…` URL.
 */
export function updaterManifest({ repo, tag, version, assetNames, signatures, publishedAt = new Date().toISOString() }) {
  const platforms = {};
  for (const name of [...assetNames].sort()) {
    const label = Object.keys(updaterPackages).find(suffix => name.endsWith(`-${version}-${suffix}`));
    if (!label) continue;
    const signature = signatures.get(`${name}.sig`);
    if (!signature) continue;
    const signed = signedVersion(signature);
    if (signed !== version) {
      throw new Error(`${name}.sig is bound to ${signed ?? 'no version'}, not ${version}. Sign with \`tauri signer sign --app-version ${version}\` (Tauri CLI 2.12 or newer).`);
    }
    const key = updaterPackages[label];
    if (platforms[key]) throw new Error(`Two updater packages for ${key}.`);
    platforms[key] = { signature: signature.trim(), url: `https://github.com/${repo}/releases/download/${encodeURIComponent(tag)}/${encodeURIComponent(name)}` };
  }
  return { version, notes: `https://github.com/${repo}/releases/tag/${tag}`, pub_date: publishedAt, platforms };
}

/** Writes and uploads `latest.json` for a release whose packages and signatures are uploaded. */
export function publishUpdaterManifest({ repo, tag, execute = (command, args) => execFileSync(command, args, { encoding: 'utf8' }) }) {
  const version = tag.replace(/^v/, '');
  const release = JSON.parse(execute('gh', ['release', 'view', tag, '--repo', repo, '--json', 'assets']));
  const assetNames = release.assets.map(asset => asset.name);
  const directory = mkdtempSync(join(tmpdir(), 'dogcodes-updater-'));
  try {
    const signatures = new Map();
    for (const name of assetNames.filter(name => name.endsWith('.sig'))) {
      execute('gh', ['release', 'download', tag, '--repo', repo, '--pattern', name, '--dir', directory, '--clobber']);
      signatures.set(name, readFileSync(join(directory, name), 'utf8'));
    }
    const manifest = updaterManifest({ repo, tag, version, assetNames, signatures });
    if (!Object.keys(manifest.platforms).length) throw new Error(`${tag} has no signed updater packages.`);
    const file = join(directory, manifestName);
    writeFileSync(file, `${JSON.stringify(manifest, null, 2)}\n`);
    execute('gh', ['release', 'upload', tag, file, '--repo', repo, '--clobber']);
    return manifest;
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const [repo, tag] = process.argv.slice(2);
  if (!repo || !tag) {
    console.error('Usage: node scripts/updater-manifest.mjs OWNER/REPO vX.Y.Z');
    process.exit(2);
  }
  const manifest = publishUpdaterManifest({ repo, tag });
  console.log(`Uploaded ${manifestName} for ${Object.keys(manifest.platforms).join(', ')}.`);
}
