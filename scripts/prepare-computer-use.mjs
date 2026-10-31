import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, readFileSync, writeFileSync, copyFileSync, cpSync, chmodSync } from 'node:fs';
import { resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { execFileSync } from 'node:child_process';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
export const version = '0.38.0';
const sha256 = bytes => createHash('sha256').update(bytes).digest('hex');
// Fixed official release checksums; do not resolve "latest" at build time.
const releases = {
  'win32-x64': ['windows-x86_64-binary.zip', '5edaa74d0afa1b55a9668dbc4f32736507bd2bd4d75b18ca1dc8881bb12402b5'],
  'win32-arm64': ['windows-arm64-binary.zip', 'd25c8ebfffb34e4c095ea170baf47b87c35dc0d1116f42e46b39df2bcbff8700'],
  'darwin-universal': ['darwin-universal-binary.tar.gz', '2bb8dc4a7e69185ffd04c4a9c0eab590bd2c54b72c2c1be13ab675be4f487732'],
  'linux-x64': ['linux-x86_64-binary.tar.gz', '31ab9709dd91ce73677e49b9f3e75c6f0d3e4d9625c286c9e58262bfbea698dd'],
  'linux-arm64': ['linux-arm64-binary.tar.gz', '80c9ae66bc7322052dd426c6133c96086b33894bf06c972bca8215384400b33d'],
};
const targets = {
  'x86_64-pc-windows-msvc': ['win32', 'x64'], 'aarch64-pc-windows-msvc': ['win32', 'arm64'],
  'x86_64-apple-darwin': ['darwin', 'x64'], 'aarch64-apple-darwin': ['darwin', 'arm64'],
  'universal-apple-darwin': ['darwin', 'universal'],
  'x86_64-unknown-linux-gnu': ['linux', 'x64'], 'aarch64-unknown-linux-gnu': ['linux', 'arm64'],
};

export function computerUseDistribution({ platform = process.platform, arch = process.arch, target } = {}) {
  target ??= Object.entries(targets).find(([, value]) => value[0] === platform && value[1] === arch)?.[0];
  const selected = targets[target];
  if (!selected) throw new Error('Unsupported computer-use build target: ' + (target ?? platform + '/' + arch));
  const [os, cpu] = selected;
  const [suffix, digest] = releases[os + '-' + (os === 'darwin' ? 'universal' : cpu)];
  const executable = os === 'win32' ? 'cua-driver.exe' : 'cua-driver';
  const binaries = os === 'win32'
    ? [executable, 'cua-driver-uia.exe', 'cua-cursor-theme.exe'] : [executable, 'cua-cursor-theme'];
  return { target, platform: os, arch: os === 'darwin' ? 'universal' : cpu,
    archive: 'cua-driver-rs-' + version + '-' + suffix, digest, executable, binaries };
}

/** Separate target directories prevent cross-builds from overwriting host tools. */
export async function prepareComputerUse({ projectRoot = root, platform = process.platform, arch = process.arch, target, signingIdentity = '-', requireSigningIdentity = false } = {}) {
  const distribution = computerUseDistribution({ platform, arch, target });
  const { archive, digest, binaries } = distribution;
  if (distribution.platform === 'darwin' && requireSigningIdentity && (!signingIdentity || signingIdentity === '-')) {
    throw new Error('Signed macOS computer-use helpers require APPLE_SIGNING_IDENTITY or a configured signing identity.');
  }
  const cache = resolve(projectRoot, '.local/computer-use');
  mkdirSync(cache, { recursive: true });
  const file = resolve(cache, archive);
  if (!existsSync(file) || sha256(readFileSync(file)) !== digest) {
    const response = await fetch('https://github.com/trycua/cua/releases/download/cua-driver-rs-v' + version + '/' + archive);
    if (!response.ok) throw new Error('Cua Driver download failed (' + response.status + ').');
    const bytes = Buffer.from(await response.arrayBuffer());
    if (sha256(bytes) !== digest) throw new Error('Cua Driver archive checksum mismatch.');
    writeFileSync(file, bytes);
  }
  const runtime = resolve(cache, 'runtime-' + distribution.target);
  mkdirSync(runtime, { recursive: true });
  execFileSync(process.platform === 'win32' ? 'tar.exe' : 'tar', ['-xf', file, '-C', runtime], { windowsHide: true });
  const destination = resolve(projectRoot, 'apps/desktop/src-tauri/resources/computer-use', distribution.target);
  mkdirSync(destination, { recursive: true });
  const files = {};
  for (const name of binaries) {
    const output = resolve(destination, name);
    const source = resolve(runtime, name);
    // Windows locks a running executable. Reusing the same pinned bytes also
    // lets packaging and startup coexist with an active computer-use task.
    if (!existsSync(output) || sha256(readFileSync(output)) !== sha256(readFileSync(source))) {
      copyFileSync(source, output);
    }
    if (distribution.platform !== 'win32') chmodSync(output, 0o755);
    if (distribution.platform === 'darwin' && process.platform === 'darwin') {
      const flags = signingIdentity === '-' ? [] : ['--options', 'runtime', '--timestamp'];
      execFileSync('codesign', ['--force', '--sign', signingIdentity, ...flags, output], { stdio: 'pipe' });
    }
    // macOS resources are signed before Tauri copies them. Hash the shipped
    // bytes, after signing, so launch verification still succeeds in the app.
    files[name] = sha256(readFileSync(output));
  }
  if (distribution.platform === 'linux' && existsSync(resolve(runtime, 'wayland-helper'))) {
    cpSync(resolve(runtime, 'wayland-helper'), resolve(destination, 'wayland-helper'), { recursive: true });
  }
  writeFileSync(resolve(destination, 'manifest.json'), JSON.stringify({ version, target: distribution.target,
    platform: distribution.platform, arch: distribution.arch, executable: distribution.executable,
    archiveSha256: digest, files }, null, 2));
  copyFileSync(resolve(projectRoot, 'scripts/licenses/cua-driver-MIT.txt'), resolve(destination, 'LICENSE.txt'));
  return destination;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  prepareComputerUse({ target: process.argv[2] }).then(path => console.log(path)).catch(error => { console.error(error.message); process.exitCode = 1; });
}
