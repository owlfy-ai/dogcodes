import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { test } from 'node:test';
import { files, opensshDistribution, prepareOpenSsh, version } from './prepare-openssh.mjs';

const target = 'x86_64-pc-windows-msvc';
const project = () => mkdtempSync(join(tmpdir(), 'dogcodes-openssh-'));
const unpack = (names, folder = 'OpenSSH-Win64') => (_file, destination) => {
  mkdirSync(resolve(destination, folder), { recursive: true });
  for (const name of names) writeFileSync(resolve(destination, folder, name), name);
};

test('only Windows targets carry a client', async () => {
  assert.equal(opensshDistribution(target).archive, 'OpenSSH-Win64.zip');
  assert.equal(opensshDistribution('aarch64-pc-windows-msvc').archive, 'OpenSSH-ARM64.zip');
  for (const other of ['aarch64-apple-darwin', 'x86_64-unknown-linux-gnu', undefined]) {
    assert.equal(opensshDistribution(other), null);
    assert.equal(await prepareOpenSsh({ target: other, fetcher: () => assert.fail('nothing to download') }), null);
  }
});

test('the version is pinned and never resolved at build time', () => {
  assert.match(version, /^\d+\.\d+\.\d+\.\d+p\d+/);
  assert.match(opensshDistribution(target).digest, /^[0-9a-f]{64}$/);
});

test('a download that does not match the pinned checksum is refused', async () => {
  const root = project();
  try {
    await assert.rejects(
      prepareOpenSsh({ projectRoot: root, target, fetcher: async () => new Response('tampered'), extract: unpack(files) }),
      /checksum mismatch/,
    );
    assert.equal(existsSync(resolve(root, '.local/openssh', version, 'OpenSSH-Win64.zip')), false);
    assert.equal(existsSync(resolve(root, 'apps/desktop/src-tauri/resources/openssh')), false);
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test('a failed download is reported', async () => {
  const root = project();
  try {
    await assert.rejects(
      prepareOpenSsh({ projectRoot: root, target, fetcher: async () => new Response('', { status: 503 }), extract: unpack(files) }),
      /download failed \(503\)/,
    );
  } finally { rmSync(root, { recursive: true, force: true }); }
});

const archive = 'archive bytes';
const verified = { folder: 'OpenSSH-Win64', archive: 'OpenSSH-Win64.zip', digest: createHash('sha256').update(archive).digest('hex') };

test('only the client, its helpers and its licenses are packaged, replacing what was there', async () => {
  const root = project();
  try {
    const stale = resolve(root, 'apps/desktop/src-tauri/resources/openssh/stale.exe');
    mkdirSync(resolve(stale, '..'), { recursive: true });
    writeFileSync(stale, 'old');
    const destination = await prepareOpenSsh({ projectRoot: root, target, distribution: verified,
      fetcher: async () => new Response(archive), extract: unpack([...files, 'sshd.exe', 'install-sshd.ps1']) });
    assert.deepEqual(readdirSync(destination).sort(), [...files].sort());
    assert.equal(readFileSync(resolve(destination, 'ssh.exe'), 'utf8'), 'ssh.exe');
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test('an archive without an expected file fails the build', async () => {
  const root = project();
  try {
    await assert.rejects(
      prepareOpenSsh({ projectRoot: root, target, distribution: verified,
        fetcher: async () => new Response(archive), extract: unpack(['ssh.exe']) }),
      /missing libcrypto\.dll/,
    );
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test('the cached archive is used without downloading again', async () => {
  const root = project();
  try {
    const cache = resolve(root, '.local/openssh', version);
    mkdirSync(cache, { recursive: true });
    writeFileSync(resolve(cache, 'OpenSSH-Win64.zip'), archive);
    await prepareOpenSsh({ projectRoot: root, target, distribution: verified, fetcher: () => assert.fail('no download'), extract: unpack(files) });
  } finally { rmSync(root, { recursive: true, force: true }); }
});
