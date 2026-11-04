import { execFileSync } from 'node:child_process';
import { chmodSync, copyFileSync, mkdirSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const linux = ['x86_64-unknown-linux-musl', 'aarch64-unknown-linux-musl'];
const mac = ['x86_64-apple-darwin', 'aarch64-apple-darwin'];

/** Rust's bundled linker reaches Linux from any host; Apple targets need an Apple SDK. */
export function shimTargets({ platform = process.platform } = {}) {
  return platform === 'darwin' ? [...linux, ...mac] : linux;
}

export function hostTarget({ platform = process.platform, arch = process.arch } = {}) {
  const cpu = arch === 'arm64' ? 'aarch64' : 'x86_64';
  if (platform === 'darwin') return `${cpu}-apple-darwin`;
  if (platform === 'linux') return `${cpu}-unknown-linux-musl`;
  return null;
}

/** Cargo reads a target's linker from a variable named after the target. */
export function linkerVariable(target) {
  return `CARGO_TARGET_${target.toUpperCase().replaceAll('-', '_')}_LINKER`;
}

export function prepareShims({ projectRoot = root, targets = shimTargets(), signingIdentity = '-', requireSigningIdentity = false, run = execFileSync } = {}) {
  if (process.platform === 'darwin' && requireSigningIdentity && (!signingIdentity || signingIdentity === '-')) {
    throw new Error('Signed macOS shims require APPLE_SIGNING_IDENTITY or a configured signing identity.');
  }
  const destination = resolve(projectRoot, 'apps/desktop/src-tauri/resources/shim');
  run('rustup', ['target', 'add', ...targets], { stdio: 'inherit', windowsHide: true });
  for (const target of targets) {
    const env = { ...process.env };
    if (target.endsWith('-musl')) env[linkerVariable(target)] = 'rust-lld';
    run('cargo', ['build', '--release', '--locked', '-p', 'target-shim', '--target', target], { cwd: projectRoot, env, stdio: 'inherit', windowsHide: true });
    const folder = resolve(destination, target);
    mkdirSync(folder, { recursive: true });
    const output = resolve(folder, 'dogcodes-shim');
    copyFileSync(resolve(projectRoot, 'target', target, 'release/dogcodes-shim'), output);
    chmodSync(output, 0o755);
    if (target.endsWith('-apple-darwin') && process.platform === 'darwin') {
      const flags = signingIdentity === '-' ? [] : ['--options', 'runtime', '--timestamp'];
      run('codesign', ['--force', '--sign', signingIdentity, ...flags, output], { stdio: 'pipe' });
    }
  }
  return destination;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  // `--host` is enough to connect to this kind of machine while developing.
  const host = process.argv[2] === '--host' ? hostTarget() : null;
  try {
    console.log(prepareShims(host ? { targets: [host] } : {}));
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
