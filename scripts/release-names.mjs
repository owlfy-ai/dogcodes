import { existsSync, readdirSync, renameSync } from 'node:fs';
import { join } from 'node:path';

// Tauri names packages from the product, version and CPU. Release downloads use
// the platform words users can match to their computer.
const labels = {
  'aarch64.dmg': 'macOS-AppleSilicon.dmg',
  'x64.dmg': 'macOS-Intel.dmg',
  'x86_64.dmg': 'macOS-Intel.dmg',
  'universal.dmg': 'macOS-Universal.dmg',
  'x64-setup.exe': 'Windows-x64-Setup.exe',
  'arm64-setup.exe': 'Windows-arm64-Setup.exe',
  'amd64.deb': 'Linux-x64.deb',
  'arm64.deb': 'Linux-arm64.deb',
  'amd64.AppImage': 'Linux-x64.AppImage',
  'aarch64.AppImage': 'Linux-arm64.AppImage',
};

export function releaseInstallerName(fileName) {
  const match = /^(?<product>.+)_(?<version>\d+\.\d+\.\d+)_(?<arch>[^.]+)\.(?<ext>dmg|exe|deb|AppImage)$/.exec(fileName);
  if (!match) return null;
  const { product, version, arch, ext } = match.groups;
  const label = labels[`${arch}.${ext}`];
  if (!label) return null;
  return `${product}-${version}-${label}`;
}

export function macReleaseDmgName(productName, version, target) {
  const arch = {
    'aarch64-apple-darwin': 'aarch64',
    'x86_64-apple-darwin': 'x86_64',
    'universal-apple-darwin': 'universal',
  }[target];
  if (!arch) throw new Error('Keychain signing requires a macOS Rust target.');
  return releaseInstallerName(`${productName}_${version}_${arch}.dmg`);
}

export function renameReleaseInstallers(bundleDirectory) {
  if (!existsSync(bundleDirectory)) return [];
  const renamed = [];
  for (const folder of ['dmg', 'nsis', 'deb', 'appimage']) {
    const directory = join(bundleDirectory, folder);
    if (!existsSync(directory)) continue;
    for (const name of readdirSync(directory)) {
      const next = releaseInstallerName(name);
      if (!next || next === name) continue;
      const destination = join(directory, next);
      if (existsSync(destination)) throw new Error(`Release installer already exists: ${next}`);
      renameSync(join(directory, name), destination);
      renamed.push(next);
    }
  }
  return renamed;
}
