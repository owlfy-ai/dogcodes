import assert from 'node:assert/strict';
import { readFileSync, writeFileSync } from 'node:fs';
import { basename, resolve } from 'node:path';
import { test } from 'node:test';
import { macUpdaterArchiveName, publishUpdaterManifest, signedVersion, updaterManifest } from './updater-manifest.mjs';

// Test-only signatures shared with the Rust updater tests: one bound to 0.2.0, one without a version.
const fixtures = resolve(import.meta.dirname, '../apps/desktop/src-tauri/src/app_update/fixtures');
const bound = readFileSync(resolve(fixtures, 'package.bin.sig'), 'utf8');
const unbound = readFileSync(resolve(fixtures, 'package-unversioned.bin.sig'), 'utf8');
const repo = 'owner/app', tag = 'v0.2.0', version = '0.2.0';
const release = [
  'dog.codes-0.2.0-macOS-AppleSilicon.dmg',
  'dog.codes-0.2.0-macOS-AppleSilicon.app.tar.gz', 'dog.codes-0.2.0-macOS-AppleSilicon.app.tar.gz.sig',
  'dog.codes-0.2.0-macOS-Intel.dmg',
  'dog.codes-0.2.0-macOS-Intel.app.tar.gz', 'dog.codes-0.2.0-macOS-Intel.app.tar.gz.sig',
  'dog.codes-0.2.0-Windows-x64-Setup.exe', 'dog.codes-0.2.0-Windows-x64-Setup.exe.sig',
  'SHA256SUMS.txt', 'signing-macos.json',
];
const signaturesFor = (names, signature = bound) => new Map(names.filter(name => name.endsWith('.sig')).map(name => [name, signature]));

test('reads the version a signature was made for', () => {
  assert.equal(signedVersion(bound), '0.2.0');
  assert.equal(signedVersion(unbound), null);
});

test('picks one signed package per platform from the release assets', () => {
  const manifest = updaterManifest({ repo, tag, version, assetNames: release, signatures: signaturesFor(release), publishedAt: '2026-11-14T00:00:00Z' });
  assert.deepEqual(Object.keys(manifest.platforms).sort(), ['darwin-aarch64', 'darwin-x86_64', 'windows-x86_64']);
  assert.equal(manifest.platforms['darwin-aarch64'].url, 'https://github.com/owner/app/releases/download/v0.2.0/dog.codes-0.2.0-macOS-AppleSilicon.app.tar.gz');
  assert.equal(manifest.platforms['windows-x86_64'].url, 'https://github.com/owner/app/releases/download/v0.2.0/dog.codes-0.2.0-Windows-x64-Setup.exe');
  assert.equal(manifest.platforms['darwin-x86_64'].signature, bound.trim());
  assert.equal(manifest.version, '0.2.0');
  assert.equal(manifest.pub_date, '2026-11-14T00:00:00Z');
});

test('leaves out unsigned packages, DMGs and other versions', () => {
  const names = ['dog.codes-0.2.0-macOS-AppleSilicon.app.tar.gz', 'dog.codes-0.2.0-macOS-AppleSilicon.dmg',
    'dog.codes-0.1.9-Windows-x64-Setup.exe', 'dog.codes-0.1.9-Windows-x64-Setup.exe.sig'];
  assert.deepEqual(updaterManifest({ repo, tag, version, assetNames: names, signatures: signaturesFor(names) }).platforms, {});
});

test('refuses signatures not bound to the release version', () => {
  assert.throws(() => updaterManifest({ repo, tag, version, assetNames: release, signatures: signaturesFor(release, unbound) }), /bound to no version.*--app-version 0\.2\.0/);
  assert.throws(() => updaterManifest({ repo, tag: 'v0.3.0', version: '0.3.0', assetNames: release.map(name => name.replace('0.2.0', '0.3.0')),
    signatures: signaturesFor(release.map(name => name.replace('0.2.0', '0.3.0'))) }), /bound to 0\.2\.0, not 0\.3\.0/);
});

test('names the macOS archive like the DMG of the same target', () => {
  assert.equal(macUpdaterArchiveName('dog.codes', '0.2.0', 'aarch64-apple-darwin'), 'dog.codes-0.2.0-macOS-AppleSilicon.app.tar.gz');
  assert.equal(macUpdaterArchiveName('dog.codes', '0.2.0', 'x86_64-apple-darwin'), 'dog.codes-0.2.0-macOS-Intel.app.tar.gz');
  assert.throws(() => macUpdaterArchiveName('dog.codes', '0.2.0', 'universal-apple-darwin'));
});

test('uploads latest.json built from the signatures attached to the release', () => {
  const calls = [];
  let uploaded;
  const execute = (command, args) => {
    calls.push([command, ...args].join(' '));
    if (args[1] === 'view') return JSON.stringify({ assets: release.map(name => ({ name })) });
    if (args[1] === 'download') {
      const directory = args[args.indexOf('--dir') + 1];
      const name = args[args.indexOf('--pattern') + 1];
      writeFileSync(resolve(directory, name), bound);
      return '';
    }
    if (args[1] === 'upload') { uploaded = JSON.parse(readFileSync(args[3], 'utf8')); assert.equal(basename(args[3]), 'latest.json'); }
    return '';
  };
  const manifest = publishUpdaterManifest({ repo, tag, execute });
  assert.deepEqual(uploaded, manifest);
  assert.equal(calls.filter(call => call.includes(' download ')).length, 3);
  assert.ok(calls.at(-1).endsWith('--repo owner/app --clobber'));
});

test('does not upload a manifest without any signed package', () => {
  const execute = (command, args) => args[1] === 'view' ? JSON.stringify({ assets: [{ name: 'dog.codes-0.2.0-macOS-Intel.dmg' }] }) : assert.fail(args.join(' '));
  assert.throws(() => publishUpdaterManifest({ repo, tag, execute }), /no signed updater packages/);
});

