import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdtempSync, mkdirSync, readFileSync, writeFileSync, rmSync, existsSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { resolve, join, basename } from 'node:path';
import { execFileSync } from 'node:child_process';
import { test } from 'node:test';
import { releaseArguments, releaseVersion, githubRepository, sha256, waitForDraft, withReleaseLock,
  uploadSignedPackages, refreshChecksums, ensureTag, releaseDesktop, lockName, githubClient } from './desktop-release.mjs';

const commit = 'a'.repeat(40), tag = 'v0.1.0';
const digest = bytes => createHash('sha256').update(bytes).digest('hex');

function fakeGitHub({ serverDigests = true } = {}) {
  const assets = new Map();
  const changes = [];
  let id = 0, draft = true, body = 'User release notes.\n<!-- dogcodes-signing:start -->\nPending\n<!-- dogcodes-signing:end -->\nUser footer.';
  const metadata = asset => ({ id: asset.id, name: asset.name, size: asset.bytes.length,
    ...(serverDigests ? { digest: `sha256:${digest(asset.bytes)}` } : {}) });
  const put = (name, bytes) => {
    const asset = { id: ++id, name, bytes: Buffer.from(bytes) };
    assets.set(name, asset);
    return metadata(asset);
  };
  for (const name of ['dog.codes_0.1.0_aarch64.dmg', 'dog.codes_0.1.0_x64.dmg',
    'dog.codes_0.1.0_x64-setup.exe', 'dog.codes_0.1.0_amd64.deb', 'dog.codes_0.1.0_amd64.AppImage']) put(name, `ci-${name}`);
  put('SHA256SUMS.txt', 'initial-checksums');
  const client = {
    async release() { return { id: 1, draft, body, assets: [...assets.values()].map(metadata) }; },
    async run() { return { status: 'completed', conclusion: 'success', html_url: 'https://example.invalid/run' }; },
    async upload(file, replace = true) {
      assert.equal(draft, true);
      const name = basename(file);
      if (assets.has(name) && !replace) throw new Error('Asset already exists');
      changes.push(`upload:${name}`);
      put(name, readFileSync(file));
    },
    async download(asset, file) {
      const current = assets.get(asset.name);
      assert.equal(current.id, asset.id);
      writeFileSync(file, current.bytes);
    },
    async remove(asset) {
      if (assets.get(asset.name)?.id === asset.id) { changes.push(`delete:${asset.name}`); assets.delete(asset.name); }
    },
    async body(release, text) { assert.equal(draft, true); body = text; changes.push('body'); },
  };
  return { client, assets, put, changes, publish: () => { draft = false; } };
}

async function fixture(work) {
  const root = mkdtempSync(resolve(tmpdir(), 'dogcodes-release-'));
  async function receipt(platform) {
    const names = platform === 'darwin' ? ['dog.codes_0.1.0_aarch64.dmg', 'dog.codes_0.1.0_x86_64.dmg'] : ['dog.codes_0.1.0_x64-setup.exe'];
    const assets = [];
    for (const name of names) {
      const path = resolve(root, name);
      writeFileSync(path, `signed-${name}`);
      assets.push({ name, path, size: readFileSync(path).length, sha256: await sha256(path) });
    }
    return { schemaVersion: 1, tag, commit, platform, assets };
  }
  const stage = name => { const path = join(root, name); mkdirSync(path); return path; };
  try { return await work({ root, receipt, stage }); }
  finally { rmSync(root, { recursive: true, force: true }); }
}

test('release versions must agree, and origin must be a GitHub URL without embedded credentials', async () => fixture(({ root }) => {
  mkdirSync(resolve(root, 'apps/desktop/src-tauri'), { recursive: true });
  for (const path of ['package.json', 'apps/desktop/package.json', 'apps/desktop/src-tauri/tauri.conf.json']) writeFileSync(resolve(root, path), '{"version":"0.1.0"}');
  writeFileSync(resolve(root, 'Cargo.toml'), '[workspace.package]\nversion = "0.1.0"\n');
  assert.equal(releaseVersion(root), '0.1.0');
  writeFileSync(resolve(root, 'apps/desktop/package.json'), '{"version":"0.2.0"}');
  assert.throws(() => releaseVersion(root), /four release version/);
  for (const remote of ['https://github.com/example/product.git', 'git@github.com:example/product.git', 'ssh://git@github.com/example/product.git']) assert.equal(githubRepository(remote), 'example/product');
  for (const remote of ['https://token@github.com/example/product.git', 'https://example.invalid/product', 'https://github.com/example/product/other']) assert.throws(() => githubRepository(remote));
  assert.deepEqual(releaseArguments(['--upload-only']), { check: false, uploadOnly: true });
  assert.throws(() => releaseArguments(['--check', '--upload-only']));
  assert.throws(() => releaseArguments(['--publish']));
}));

test('dirty worktrees stop before any authentication, build or remote mutation', async () => fixture(async ({ root }) => {
  mkdirSync(resolve(root, 'apps/desktop/src-tauri'), { recursive: true });
  for (const path of ['package.json', 'apps/desktop/package.json', 'apps/desktop/src-tauri/tauri.conf.json']) writeFileSync(resolve(root, path), '{"version":"0.1.0"}');
  writeFileSync(resolve(root, 'Cargo.toml'), '[workspace.package]\nversion = "0.1.0"\n');
  const git = args => execFileSync('git', args, { cwd: root, stdio: 'pipe' });
  git(['init', '--quiet']); git(['add', '.']);
  git(['-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', '-c', 'commit.gpgSign=false', 'commit', '--quiet', '-m', 'Fixture']);
  writeFileSync(resolve(root, 'uncommitted.txt'), 'pending');
  await assert.rejects(releaseDesktop({ root, platform: 'darwin', args: ['--check'] }), /Commit the intended source changes/);
  assert.equal(git(['tag', '--list']).toString(), '');
  assert.equal(existsSync(resolve(root, 'release')), false);
}));

test('existing version tags are reused only for the same commit, never moved', async () => {
  const calls = [];
  const git = async args => { calls.push(args); return `${'b'.repeat(40)}\trefs/tags/${tag}^{}\n`; };
  await assert.rejects(ensureTag({ tag, commit, git }), /another commit/);
  assert.equal(calls.length, 1);
  calls.length = 0;
  await ensureTag({ tag, commit, git: async args => { calls.push(args); return `${commit}\trefs/tags/${tag}^{}\n`; } });
  assert.equal(calls.length, 1);
});

test('a concurrent same-commit tag push is recovered without force-pushing', async () => {
  const calls = [];
  let remote;
  await ensureTag({ tag, commit, git: async args => {
    calls.push(args);
    if (args[0] === 'ls-remote') {
      if (remote) return `${remote}\trefs/tags/${tag}^{}\n`;
      throw Object.assign(new Error('missing'), { exitCode: 2 });
    }
    if (args[0] === 'rev-parse') throw Object.assign(new Error('missing'), { exitCode: 128 });
    if (args[0] === 'push') { remote = commit; throw new Error('another machine pushed first'); }
    return '';
  } });
  assert.equal(calls.some(args => args.includes('--force')), false);
  assert.equal(calls.filter(args => args[0] === 'tag').length, 1);
});

test('tag preflight performs no tag creation or push', async () => {
  const calls = [];
  await ensureTag({ tag, commit, checkOnly: true, git: async args => {
    calls.push(args);
    if (args[0] === 'ls-remote') throw Object.assign(new Error('missing'), { exitCode: 2 });
    if (args[0] === 'rev-parse') throw Object.assign(new Error('missing'), { exitCode: 128 });
    assert.fail('Preflight must be read-only');
  } });
  assert.equal(calls.length, 2);
});

test('failed CI and published releases stop the upload flow', async () => {
  await assert.rejects(waitForDraft({ release: async () => ({ draft: true }),
    run: async () => ({ status: 'completed', conclusion: 'failure', html_url: 'https://example.invalid/run' }) }), /Release CI failure/);
  await assert.rejects(waitForDraft({ release: async () => ({ draft: false }) }), /Published releases/);
  let rounds = 0;
  const release = await waitForDraft({ release: async () => ({ draft: true }), run: async () => ({ status: ++rounds === 1 ? 'in_progress' : 'completed', conclusion: 'success' }) }, commit,
    { wait: async () => {}, log() {}, attempts: 3 });
  assert.equal(release.draft, true);
});

test('Mac upload replaces CI naming variants, keeps Windows/Linux, and hashes the actual asset bytes', async () => fixture(async ({ receipt, stage }) => {
  const hub = fakeGitHub({ serverDigests: false });
  const ready = await uploadSignedPackages(hub.client, await receipt('darwin'), stage('mac'));
  assert.equal(ready, false);
  assert.equal(hub.assets.has('dog.codes_0.1.0_x64.dmg'), false);
  assert.equal(hub.assets.get('dog.codes_0.1.0_x64-setup.exe').bytes.toString(), 'ci-dog.codes_0.1.0_x64-setup.exe');
  const lines = hub.assets.get('SHA256SUMS.txt').bytes.toString().trim().split('\n');
  assert.equal(lines.length, 5);
  for (const line of lines) {
    const [sha, name] = line.split('  ');
    assert.equal(sha, digest(hub.assets.get(name).bytes));
  }
  assert.equal(hub.assets.has(lockName), false);
  assert.equal(JSON.parse(hub.assets.get('signing-macos.json').bytes).assets.some(asset => 'path' in asset), false);
  const release = await hub.client.release();
  assert.match(release.body, /^User release notes\./);
  assert.match(release.body, /User footer\.$/);
  assert.match(release.body, /Awaiting local signed package/);
}));

test('two computers serialize updates and produce one correct checksum set for both signed platforms', async () => fixture(async ({ receipt, stage }) => {
  const hub = fakeGitHub();
  const mac = await receipt('darwin'), windows = await receipt('win32');
  const results = await Promise.all([
    uploadSignedPackages(hub.client, mac, stage('mac'), { wait: () => new Promise(resolve => setTimeout(resolve, 1)) }),
    uploadSignedPackages(hub.client, windows, stage('windows'), { wait: () => new Promise(resolve => setTimeout(resolve, 1)) }),
  ]);
  assert.equal(results.includes(true), true);
  const release = await hub.client.release();
  assert.equal(release.draft, true);
  assert.match(release.body, /Developer ID signed and notarized/);
  assert.match(release.body, /Authenticode signed and timestamped/);
  assert.equal(hub.assets.has(lockName), false);
  for (const asset of [...mac.assets, ...windows.assets]) assert.equal(digest(hub.assets.get(asset.name).bytes), asset.sha256);
  assert.equal(hub.assets.get('SHA256SUMS.txt').bytes.toString().trim().split('\n').length, 5);
  assert.equal(hub.changes.some(change => change.includes('delete:dog.codes_0.1.0_amd64')), false);
}));

test('a failed upload releases its lock and can resume without replacing other platforms', async () => fixture(async ({ receipt, stage }) => {
  const hub = fakeGitHub(), mac = await receipt('darwin');
  const upload = hub.client.upload;
  hub.client.upload = async (file, replace) => {
    if (basename(file).endsWith('x86_64.dmg')) throw new Error('network failure');
    return upload(file, replace);
  };
  await assert.rejects(uploadSignedPackages(hub.client, mac, stage('failed')), /network failure/);
  assert.equal(hub.assets.has(lockName), false);
  assert.equal(hub.assets.has('signing-macos.json'), false);
  assert.equal(hub.assets.get('SHA256SUMS.txt').bytes.toString(), 'initial-checksums');
  hub.client.upload = upload;
  await uploadSignedPackages(hub.client, mac, stage('retry'));
  assert.equal(hub.assets.has('signing-macos.json'), true);
}));

test('lost lock-upload responses are recovered by ownership; foreign locks are never deleted', async () => fixture(async ({ stage }) => {
  const hub = fakeGitHub(), upload = hub.client.upload;
  hub.client.upload = async (file, replace) => {
    await upload(file, replace);
    if (basename(file) === lockName) throw new Error('lost response');
  };
  let ran = false;
  await withReleaseLock(hub.client, stage('lost-response'), async () => { ran = true; });
  assert.equal(ran, true);
  assert.equal(hub.assets.has(lockName), false);
  hub.put(lockName, JSON.stringify({ owner: 'another-computer' }));
  await assert.rejects(withReleaseLock(hub.client, stage('busy'), async () => assert.fail('must not run'), { wait: async () => {}, attempts: 2 }), /Another computer/);
  assert.equal(JSON.parse(hub.assets.get(lockName).bytes).owner, 'another-computer');
}));

test('changed remote signed bytes cannot be declared ready or replace the checksum file', async () => fixture(async ({ receipt, stage }) => {
  const hub = fakeGitHub();
  await uploadSignedPackages(hub.client, await receipt('darwin'), stage('upload'));
  const previous = hub.assets.get('SHA256SUMS.txt').bytes.toString();
  hub.put('dog.codes_0.1.0_aarch64.dmg', 'changed-after-upload');
  await assert.rejects(refreshChecksums(hub.client, { tag, commit, stage: stage('refresh') }), /receipt does not match/);
  assert.equal(hub.assets.get('SHA256SUMS.txt').bytes.toString(), previous);
}));

test('lock release between a name collision and ownership read retries normally', async () => fixture(async ({ stage }) => {
  const hub = fakeGitHub(), upload = hub.client.upload;
  let collided = false, ran = false;
  hub.client.upload = async (file, replace) => {
    if (basename(file) === lockName && !collided) {
      collided = true;
      throw new Error('Asset already exists'); // Previous owner has just removed it.
    }
    return upload(file, replace);
  };
  await withReleaseLock(hub.client, stage('released-during-collision'), async () => { ran = true; }, { wait: async () => {}, attempts: 3 });
  assert.equal(ran, true);
  assert.equal(hub.assets.has(lockName), false);
}));

test('published releases receive no lock, upload, deletion or notes mutation', async () => fixture(async ({ receipt, stage }) => {
  const hub = fakeGitHub();
  hub.publish();
  await assert.rejects(uploadSignedPackages(hub.client, await receipt('win32'), stage('published')), /Published releases/);
  assert.deepEqual(hub.changes, []);
}));

test('GitHub CLI adapter reads draft assets by release ID and claims locks without clobber', async () => {
  const calls = [];
  const client = githubClient({ root: '.', repo: 'example/product', tag, execute: async (command, args, options) => {
    calls.push({ command, args, options });
    if (args[0] === 'release' && args[1] === 'view') return '{"databaseId":42}';
    if (args[0] === 'api' && !args.includes('--method')) return '{"id":42,"draft":true,"assets":[]}';
    return '';
  } });
  assert.equal((await client.release()).draft, true);
  assert.equal(calls[1].args[1], 'repos/example/product/releases/42');
  await client.upload('/private/RELEASE_UPLOAD_LOCK.json', false);
  assert.equal(calls.at(-1).args.includes('--clobber'), false);
  await client.upload('/private/SHA256SUMS.txt');
  assert.equal(calls.at(-1).args.includes('--clobber'), true);
  await client.body({ id: 42 }, 'Notes\nwith newlines');
  assert.deepEqual(JSON.parse(calls.at(-1).options.input), { body: 'Notes\nwith newlines' });
  assert.equal(calls.at(-1).args.includes('Notes\nwith newlines'), false);
});

test('external installer replacement during hashing is detected before updating checksums', async () => fixture(async ({ stage }) => {
  const hub = fakeGitHub({ serverDigests: false });
  const download = hub.client.download;
  let replaced = false;
  hub.client.download = async (asset, file) => {
    await download(asset, file);
    if (!replaced && asset.name.endsWith('.dmg')) {
      replaced = true;
      hub.put(asset.name, 'external-replacement');
    }
  };
  await assert.rejects(refreshChecksums(hub.client, { tag, commit, stage: stage('race') }), /changed while checksums/);
  assert.equal(hub.assets.get('SHA256SUMS.txt').bytes.toString(), 'initial-checksums');
}));
