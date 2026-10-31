import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, readFileSync, writeFileSync, copyFileSync } from 'node:fs';
import { resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { execFileSync } from 'node:child_process';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
export const version = '0.38.0';
const archive = `cua-driver-rs-${version}-windows-x86_64-binary.zip`;
const digest = '5edaa74d0afa1b55a9668dbc4f32736507bd2bd4d75b18ca1dc8881bb12402b5';
const sha256 = bytes => createHash('sha256').update(bytes).digest('hex');

/** Official pinned release, including the UIA helper required by Windows. */
export async function prepareComputerUse({ projectRoot = root, platform = process.platform, arch = process.arch } = {}) {
  if (platform !== 'win32') return null; // Other platform packaging is a later phase.
  if (arch !== 'x64') throw new Error('Computer use currently packages Windows x64 only.');
  const cache = resolve(projectRoot, '.local/computer-use');
  const zip = resolve(cache, 'cua-driver.zip');
  mkdirSync(cache, { recursive: true });
  if (!existsSync(zip) || sha256(readFileSync(zip)) !== digest) {
    const response = await fetch(`https://github.com/trycua/cua/releases/download/cua-driver-rs-v${version}/${archive}`);
    if (!response.ok) throw new Error(`Cua Driver download failed (${response.status}).`);
    const bytes = Buffer.from(await response.arrayBuffer());
    if (sha256(bytes) !== digest) throw new Error('Cua Driver archive checksum mismatch.');
    writeFileSync(zip, bytes);
  }
  const runtime = resolve(cache, 'runtime');
  mkdirSync(runtime, { recursive: true });
  execFileSync('tar.exe', ['-xf', zip, '-C', runtime], { windowsHide: true });
  const destination = resolve(projectRoot, 'apps/desktop/src-tauri/resources/computer-use');
  mkdirSync(destination, { recursive: true });
  const files = {};
  for (const name of ['cua-driver.exe', 'cua-driver-uia.exe']) {
    const bytes = readFileSync(resolve(runtime, name));
    files[name] = sha256(bytes);
    copyFileSync(resolve(runtime, name), resolve(destination, name));
  }
  writeFileSync(resolve(destination, 'manifest.json'), JSON.stringify({ version, archiveSha256: digest, files }, null, 2));
  copyFileSync(resolve(projectRoot, 'scripts/licenses/cua-driver-MIT.txt'), resolve(destination, 'LICENSE.txt'));
  return destination;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  prepareComputerUse().then(path => console.log(path ?? 'Computer-use packaging is currently Windows-only.')).catch(error => { console.error(error.message); process.exitCode = 1; });
}
