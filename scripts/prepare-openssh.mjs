import { createHash } from 'node:crypto';
import { copyFileSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { execFileSync } from 'node:child_process';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
export const version = '10.4.0.0p1-Preview';
const sha256 = bytes => createHash('sha256').update(bytes).digest('hex');
// Fixed official release checksums; do not resolve "latest" at build time.
const releases = {
  'x86_64-pc-windows-msvc': ['OpenSSH-Win64', 'b67ff8becd203502190ec42ac7787c24c2cd3325ed7700485fe7a1c83d2a7957'],
  'aarch64-pc-windows-msvc': ['OpenSSH-ARM64', 'd2b1928225479fec9e0c938f527fdb5ed676446cca1bbcc5af84aca1f272842e'],
};
// The client and what it runs. Hardware keys need the helpers beside `ssh.exe`.
export const files = ['ssh.exe', 'libcrypto.dll', 'ssh-sk-helper.exe', 'ssh-pkcs11-helper.exe', 'LICENSE.txt', 'NOTICE.txt'];

/** Only Windows installers carry a client: Windows may not have one, or an old one. */
export function opensshDistribution(target) {
  const release = releases[target];
  return release ? { folder: release[0], digest: release[1], archive: `${release[0]}.zip` } : null;
}

export async function prepareOpenSsh({ projectRoot = root, target, fetcher = fetch, extract = extractArchive, distribution = opensshDistribution(target) } = {}) {
  if (!distribution) return null;
  const { archive, digest, folder } = distribution;
  const cache = resolve(projectRoot, '.local/openssh', version);
  mkdirSync(cache, { recursive: true });
  const file = resolve(cache, archive);
  if (!existsSync(file) || sha256(readFileSync(file)) !== digest) {
    const response = await fetcher(`https://github.com/PowerShell/Win32-OpenSSH/releases/download/${version}/${archive}`);
    if (!response.ok) throw new Error(`OpenSSH download failed (${response.status}).`);
    const bytes = Buffer.from(await response.arrayBuffer());
    if (sha256(bytes) !== digest) throw new Error('OpenSSH archive checksum mismatch.');
    writeFileSync(file, bytes);
  }
  const unpacked = resolve(cache, `unpacked-${folder}`);
  rmSync(unpacked, { recursive: true, force: true });
  mkdirSync(unpacked, { recursive: true });
  extract(file, unpacked);
  // Replaced whole, so a target built earlier can't leave its files behind.
  const destination = resolve(projectRoot, 'apps/desktop/src-tauri/resources/openssh');
  rmSync(destination, { recursive: true, force: true });
  mkdirSync(destination, { recursive: true });
  for (const name of files) {
    const source = resolve(unpacked, folder, name);
    if (!existsSync(source)) throw new Error(`OpenSSH archive is missing ${name}.`);
    copyFileSync(source, resolve(destination, name));
  }
  return destination;
}

function extractArchive(file, destination) {
  execFileSync(process.platform === 'win32' ? 'tar.exe' : 'tar', ['-xf', file, '-C', destination], { windowsHide: true });
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  prepareOpenSsh({ target: process.argv[2] ?? 'x86_64-pc-windows-msvc' }).then(path => console.log(path)).catch(error => {
    console.error(error.message);
    process.exitCode = 1;
  });
}
