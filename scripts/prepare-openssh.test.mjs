import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { createServer } from 'node:http';
import { once } from 'node:events';
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
      prepareOpenSsh({ projectRoot: root, target, wait: async () => {}, fetcher: async () => new Response('', { status: 503 }), extract: unpack(files) }),
      /OpenSSH download failed.*after 3 attempt\(s\): HTTP 503/,
    );
  } finally { rmSync(root, { recursive: true, force: true }); }
});

const archive = 'archive bytes';
const verified = { folder: 'OpenSSH-Win64', archive: 'OpenSSH-Win64.zip', digest: createHash('sha256').update(archive).digest('hex') };

test('a stale connection, transient HTTP error and interrupted body are retried', async () => {
  for (const failure of [
    () => { throw new TypeError('fetch failed', { cause: Object.assign(new Error('socket closed'), { code: 'ECONNRESET' }) }); },
    () => new Response('', { status: 503 }),
    () => ({ ok: true, arrayBuffer: async () => { throw new Error('terminated'); } }),
  ]) {
    const root = project();
    let calls = 0;
    const waits = [];
    try {
      await prepareOpenSsh({ projectRoot: root, target, distribution: verified, extract: unpack(files),
        wait: async ms => { waits.push(ms); },
        fetcher: async (_url, { signal }) => {
          assert.ok(signal instanceof AbortSignal);
          return ++calls === 1 ? failure() : new Response(archive);
        } });
      assert.equal(calls, 2);
      assert.deepEqual(waits, [1000]);
      assert.equal(readFileSync(resolve(root, '.local/openssh', version, verified.archive), 'utf8'), archive);
    } finally { rmSync(root, { recursive: true, force: true }); }
  }
});

test('the real Node fetch recovers after the server closes the connection', async () => {
  const root = project();
  let requests = 0;
  const server = createServer((request, response) => {
    if (++requests === 1) request.socket.destroy();
    else response.end(archive);
  });
  try {
    server.listen(0, '127.0.0.1');
    await once(server, 'listening');
    const url = `http://127.0.0.1:${server.address().port}/archive.zip`;
    const destination = await prepareOpenSsh({ projectRoot: root, target, distribution: verified,
      wait: async () => {}, extract: unpack(files),
      fetcher: (_url, options) => fetch(url, options) });
    assert.equal(requests, 2);
    assert.deepEqual(readdirSync(destination).sort(), [...files].sort());
  } finally {
    server.closeAllConnections();
    await new Promise(resolve => server.close(resolve));
    rmSync(root, { recursive: true, force: true });
  }
});

test('persistent connection failures report the asset and underlying cause without caching bytes', async () => {
  const root = project();
  const waits = [];
  let calls = 0;
  try {
    await assert.rejects(prepareOpenSsh({ projectRoot: root, target, extract: unpack(files),
      wait: async ms => { waits.push(ms); }, fetcher: async () => {
        calls++;
        throw new TypeError('fetch failed', { cause: Object.assign(new Error('socket closed'), { code: 'ECONNRESET' }) });
      } }), /OpenSSH download failed for https:.*OpenSSH-Win64.zip after 3 attempt\(s\): fetch failed: ECONNRESET: socket closed/);
    assert.equal(calls, 3);
    assert.deepEqual(waits, [1000, 2000]);
    assert.equal(existsSync(resolve(root, '.local/openssh', version, verified.archive)), false);
    assert.equal(existsSync(resolve(root, 'apps/desktop/src-tauri/resources/openssh')), false);
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test('an unpublished asset fails immediately without retrying', async () => {
  const root = project();
  try {
    await assert.rejects(prepareOpenSsh({ projectRoot: root, target,
      wait: () => assert.fail('404 must not retry'),
      fetcher: async () => new Response('', { status: 404 }), extract: unpack(files) }), /after 1 attempt\(s\): HTTP 404/);
  } finally { rmSync(root, { recursive: true, force: true }); }
});

test('a timed out transfer is aborted and retried', async () => {
  const root = project();
  let calls = 0;
  // Keep the event loop alive while AbortSignal's unref'ed timeout fires.
  const keepAlive = setInterval(() => {}, 1000);
  try {
    await prepareOpenSsh({ projectRoot: root, target, distribution: verified, timeoutMs: 10,
      wait: async () => {}, extract: unpack(files), fetcher: async (_url, { signal }) => {
        if (++calls > 1) return new Response(archive);
        return new Promise((_resolve, reject) => signal.addEventListener('abort', () => reject(signal.reason), { once: true }));
      } });
    assert.equal(calls, 2);
  } finally {
    clearInterval(keepAlive);
    rmSync(root, { recursive: true, force: true });
  }
});

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
