import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdtempSync, mkdirSync, readFileSync, writeFileSync, rmSync, existsSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { resolve, join, basename } from 'node:path';
import { execFileSync } from 'node:child_process';
import { test } from 'node:test';
import { releaseArguments, releaseVersion, githubRepository, sha256, waitForDraft, withReleaseLock,
  uploadSignedPackages, refreshChecksums, ensureTag, releaseDesktop, lockName, githubClient, verifyUpdaterFiles, ownedBy, retryGitHub } from './desktop-release.mjs';
import { testUpdaterKey } from './fixtures/updater-key.mjs';

const commit = 'a'.repeat(40), tag = 'v0.1.0';
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
const updaterKey = testUpdaterKey();
const macNames = ['dog.codes-0.1.0-macOS-AppleSilicon', 'dog.codes-0.1.0-macOS-Intel']
  .flatMap(base => [`${base}.dmg`, `${base}.app.tar.gz`, `${base}.app.tar.gz.sig`]);
const windowsNames = ['dog.codes-0.1.0-Windows-x64-Setup.exe', 'dog.codes-0.1.0-Windows-x64-Setup.exe.sig'];

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
  for (const name of ['dog.codes-0.1.0-macOS-AppleSilicon.dmg', 'dog.codes-0.1.0-macOS-Intel.dmg',
    'dog.codes-0.1.0-Windows-x64-Setup.exe']) put(name, `ci-${name}`);
  put('SHA256SUMS.txt', 'initial-checksums');
  const client = {
    repo: 'example/product',
    async release() { return { id: 1, draft, body, assets: [...assets.values()].map(metadata) }; },
    async run() { return { status: 'completed', conclusion: 'success', html_url: 'https://example.invalid/run' }; },
    async upload(file, replace = true) {
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
    async body(release, text) { body = text; changes.push('body'); },
  };
  return { client, assets, put, changes, publish: () => { draft = false; } };
}

async function fixture(work) {
  const root = mkdtempSync(resolve(tmpdir(), 'dogcodes-release-'));
  // A signing machine's local packages: installers, update archives and update signatures.
  async function receipt(platform, { signedVersion = '0.1.0', updates = true } = {}) {
    const names = (platform === 'darwin' ? macNames : windowsNames).filter(name => updates || /\.(dmg|exe)$/.test(name));
    const assets = [];
    for (const name of names) {
      const path = resolve(root, name);
      writeFileSync(path, name.endsWith('.sig') ? updaterKey.signature(Buffer.from(`signed-${name.slice(0, -4)}`), signedVersion) : `signed-${name}`);
      assets.push({ name, path, size: readFileSync(path).length, sha256: await sha256(path) });
    }
    return { schemaVersion: 2, tag, commit, platform, assets };
  }
  const stage = name => { const path = join(root, name); mkdirSync(path); return path; };
  try { return await work({ root, receipt, stage }); }
  finally { rmSync(root, { recursive: true, force: true }); }
}

test('release version comes from the Cargo workspace, and origin must be a GitHub URL without embedded credentials', async () => fixture(({ root }) => {
  writeFileSync(resolve(root, 'Cargo.toml'), '[workspace.package]\nversion = "0.1.0"\n');
  assert.equal(releaseVersion(root), '0.1.0');
  writeFileSync(resolve(root, 'package.json'), '{"version":"0.2.0"}\n');
  assert.equal(releaseVersion(root), '0.1.0');
  writeFileSync(resolve(root, 'Cargo.toml'), '[workspace.package]\nversion = "next"\n');
  assert.throws(() => releaseVersion(root), /semantic version/);
  for (const remote of ['https://github.com/example/product.git', 'git@github.com:example/product.git', 'ssh://git@github.com/example/product.git']) assert.equal(githubRepository(remote), 'example/product');
  for (const remote of ['https://token@github.com/example/product.git', 'https://example.invalid/product', 'https://github.com/example/product/other']) assert.throws(() => githubRepository(remote));
  assert.deepEqual(releaseArguments(['--upload-only']), { check: false, uploadOnly: true });
  assert.throws(() => releaseArguments(['--check', '--upload-only']));
  assert.throws(() => releaseArguments(['--publish']));
}));

test('a dirty worktree does not stop release before the remote is checked', async () => fixture(async ({ root }) => {
  writeFileSync(resolve(root, 'Cargo.toml'), '[workspace.package]\nversion = "0.1.0"\n');
  const git = args => execFileSync('git', args, { cwd: root, stdio: 'pipe' });
  git(['init', '--quiet']); git(['add', '.']);
  git(['-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', '-c', 'commit.gpgSign=false', 'commit', '--quiet', '-m', 'Fixture']);
  writeFileSync(resolve(root, 'uncommitted.txt'), 'pending');
  await assert.rejects(releaseDesktop({ root, platform: 'darwin', args: ['--check'] }), /git remote failed/);
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

test('failed CI stops the upload flow, and a published release remains updatable', async () => {
  await assert.rejects(waitForDraft({ release: async () => ({ draft: true }),
    run: async () => ({ status: 'completed', conclusion: 'failure', html_url: 'https://example.invalid/run' }) }), /Release CI failure/);
  const published = await waitForDraft({ release: async () => ({ draft: false, id: 7 }),
    run: async () => ({ status: 'completed', conclusion: 'success', html_url: 'https://example.invalid/run' }) }, commit, { wait: async () => {}, log() {}, attempts: 1 });
  assert.equal(published.draft, false);
  let rounds = 0;
  const release = await waitForDraft({ release: async () => ({ draft: true }), run: async () => ({ status: ++rounds === 1 ? 'in_progress' : 'completed', conclusion: 'success' }) }, commit,
    { wait: async () => {}, log() {}, attempts: 3 });
  assert.equal(release.draft, true);
});

test('Mac upload replaces CI naming variants, keeps Windows/Linux, and hashes the actual asset bytes', async () => fixture(async ({ receipt, stage }) => {
  const hub = fakeGitHub({ serverDigests: false });
  const ready = await uploadSignedPackages(hub.client, await receipt('darwin'), stage('mac'));
  assert.equal(ready, false);
  assert.equal(hub.assets.get('dog.codes-0.1.0-macOS-Intel.dmg').bytes.toString(), 'signed-dog.codes-0.1.0-macOS-Intel.dmg');
  assert.equal(hub.assets.get('dog.codes-0.1.0-Windows-x64-Setup.exe').bytes.toString(), 'ci-dog.codes-0.1.0-Windows-x64-Setup.exe');
  const lines = hub.assets.get('SHA256SUMS.txt').bytes.toString().trim().split('\n');
  assert.equal(lines.length, 7);
  assert.deepEqual(Object.keys(JSON.parse(hub.assets.get('latest.json').bytes).platforms), ['darwin-aarch64', 'darwin-x86_64']);
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
  assert.match(release.body, /\| In-app updates \| Awaiting signed update packages \|/);
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
  assert.equal(hub.assets.get('SHA256SUMS.txt').bytes.toString().trim().split('\n').length, 8);
  assert.equal(hub.changes.some(change => change.includes('delete:dog.codes-0.1.0-Linux')), false);
  assert.match(release.body, /\| In-app updates \| Signed update packages for macOS ARM64 \/ Intel and Windows x64 \|/);
  const latest = JSON.parse(hub.assets.get('latest.json').bytes);
  assert.equal(latest.version, '0.1.0');
  assert.deepEqual(Object.keys(latest.platforms), ['darwin-aarch64', 'darwin-x86_64', 'windows-x86_64']);
  assert.equal(latest.platforms['windows-x86_64'].url, 'https://github.com/example/product/releases/download/v0.1.0/dog.codes-0.1.0-Windows-x64-Setup.exe');
  assert.equal(latest.platforms['darwin-aarch64'].signature, hub.assets.get('dog.codes-0.1.0-macOS-AppleSilicon.app.tar.gz.sig').bytes.toString().trim());
}));

test('a failed upload releases its lock and can resume without replacing other platforms', async () => fixture(async ({ receipt, stage }) => {
  const hub = fakeGitHub(), mac = await receipt('darwin');
  const upload = hub.client.upload;
  hub.client.upload = async (file, replace) => {
    if (basename(file).endsWith('macOS-Intel.dmg')) throw new Error('network failure');
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
  hub.put('dog.codes-0.1.0-macOS-AppleSilicon.dmg', 'changed-after-upload');
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

test('published releases accept signed packages, checksums and notes', async () => fixture(async ({ receipt, stage }) => {
  const hub = fakeGitHub();
  hub.publish();
  const ready = await uploadSignedPackages(hub.client, await receipt('win32'), stage('published'));
  assert.equal(ready, false);
  assert.equal((await hub.client.release()).draft, false);
  assert.equal(hub.assets.get('dog.codes-0.1.0-Windows-x64-Setup.exe').bytes.toString(), 'signed-dog.codes-0.1.0-Windows-x64-Setup.exe');
  assert.match((await hub.client.release()).body, /Authenticode signed and timestamped/);
  assert.equal(hub.assets.has(lockName), false);
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
  await client.download({ name: 'signing-win32.json' }, '/private/stage/signing-win32.json');
  assert.equal(calls.at(-1).args.includes('--clobber'), true);
  await client.body({ id: 42, tag_name: tag, target_commitish: commit }, 'Notes\nwith newlines');
  assert.deepEqual(JSON.parse(calls.at(-1).options.input), { body: 'Notes\nwith newlines', tag_name: tag, target_commitish: commit });
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

test('each signing machine owns its installers, update archives and signatures', () => {
  for (const name of macNames) assert.equal(ownedBy('darwin', name), true, name);
  for (const name of windowsNames) assert.equal(ownedBy('win32', name), true, name);
  for (const name of [...macNames, 'SHA256SUMS.txt', 'latest.json']) assert.equal(ownedBy('win32', name), false, name);
  for (const name of [...windowsNames, 'signing-macos.json']) assert.equal(ownedBy('darwin', name), false, name);
});

test('both platforms without update signatures are not reported ready', async () => fixture(async ({ receipt, stage }) => {
  const hub = fakeGitHub();
  await uploadSignedPackages(hub.client, await receipt('darwin', { updates: false }), stage('mac'));
  await assert.rejects(uploadSignedPackages(hub.client, await receipt('win32', { updates: false }), stage('windows')), /missing in-app update signatures/);
  assert.equal(hub.assets.has('latest.json'), false);
  assert.equal(hub.assets.has(lockName), false);
}));

test('update signatures for another version or without a package stop the checksum refresh', async () => fixture(async ({ receipt, stage }) => {
  const hub = fakeGitHub();
  await assert.rejects(uploadSignedPackages(hub.client, await receipt('win32', { signedVersion: '0.0.9' }), stage('stale')), /bound to 0\.0\.9, not 0\.1\.0/);
  assert.equal(hub.assets.get('SHA256SUMS.txt').bytes.toString(), 'initial-checksums');
  assert.equal(hub.assets.has('latest.json'), false);
  const orphan = fakeGitHub();
  orphan.put('dog.codes-0.1.0-macOS-Intel.app.tar.gz.sig', updaterKey.signature(Buffer.from('x'), '0.1.0'));
  await assert.rejects(refreshChecksums(orphan.client, { tag, commit, stage: stage('orphan') }), /incomplete/);
}));

test('GitHub rate limits are retried and credential failures are not', async () => {
  let calls = 0;
  const result = await retryGitHub(async () => {
    calls += 1;
    if (calls === 1) throw Object.assign(new Error('gh release failed: HTTP 502'), { detail: 'HTTP 502: Bad Gateway' });
    return 'ok';
  }, { wait: async () => {}, attempts: 3, log() {} });
  assert.equal(result, 'ok');
  assert.equal(calls, 2);
  calls = 0;
  const limited = Object.assign(new Error('gh release failed: HTTP 403: secondary rate limit exceeded'), { detail: 'You have exceeded a secondary rate limit. Please wait a few minutes.' });
  await assert.rejects(retryGitHub(async () => { calls += 1; throw limited; }, { wait: async () => {}, attempts: 3, log() {} }), /secondary rate limit/);
  assert.equal(calls, 3);
  calls = 0;
  const denied = Object.assign(new Error('gh release failed: HTTP 401: Bad credentials'), { detail: 'HTTP 401: Bad credentials' });
  await assert.rejects(retryGitHub(async () => { calls += 1; throw denied; }, { wait: async () => {}, attempts: 4, log() {} }), /Bad credentials/);
  assert.equal(calls, 1);
});

test('a repeated upload keeps matching GitHub assets in place', async () => fixture(async ({ receipt, stage }) => {
  const hub = fakeGitHub();
  const mac = await receipt('darwin');
  await uploadSignedPackages(hub.client, mac, stage('first'));
  const before = new Map([...hub.assets].map(([name, asset]) => [name, asset.id]));
  hub.changes.length = 0;
  await uploadSignedPackages(hub.client, mac, stage('second'));
  for (const name of [...mac.assets.map(asset => asset.name), 'SHA256SUMS.txt', 'latest.json', 'signing-macos.json']) {
    assert.equal(hub.assets.get(name).id, before.get(name), name);
  }
  assert.equal(hub.changes.some(change => change.startsWith('upload:') && change !== `upload:${lockName}`), false);
  assert.equal(hub.changes.includes('body'), false);
}));

test('a Mac upload replaces earlier update files of its own platform only', async () => fixture(async ({ receipt, stage }) => {
  const hub = fakeGitHub();
  hub.put('dog.codes-0.1.0-macOS-AppleSilicon-old.app.tar.gz', 'stale');
  await uploadSignedPackages(hub.client, await receipt('darwin'), stage('mac'));
  assert.equal(hub.assets.has('dog.codes-0.1.0-macOS-AppleSilicon-old.app.tar.gz'), false);
  assert.equal(hub.assets.has('dog.codes-0.1.0-Windows-x64-Setup.exe'), true);
}));

test('local update packages must carry a signature from the committed key for this version', async () => fixture(async ({ root, receipt }) => {
  mkdirSync(resolve(root, 'apps/desktop/src-tauri'), { recursive: true });
  const config = pubkey => writeFileSync(resolve(root, 'apps/desktop/src-tauri/tauri.conf.json'), JSON.stringify({ plugins: { updater: { pubkey } } }));
  config(updaterKey.pubkey);
  const mac = await receipt('darwin');
  verifyUpdaterFiles({ root, version: '0.1.0', assets: mac.assets });
  assert.throws(() => verifyUpdaterFiles({ root, version: '0.1.1', assets: mac.assets }), /bound to 0\.1\.0, not 0\.1\.1/);
  assert.throws(() => verifyUpdaterFiles({ root, version: '0.1.0', assets: mac.assets.filter(asset => !asset.name.endsWith('.sig')) }), /exactly one signature/);
  const unbound = await receipt('win32', { signedVersion: null });
  assert.throws(() => verifyUpdaterFiles({ root, version: '0.1.0', assets: unbound.assets }), /bound to no version/);
  config(testUpdaterKey().pubkey);
  assert.throws(() => verifyUpdaterFiles({ root, version: '0.1.0', assets: mac.assets }), /different key/);
  config('');
  assert.throws(() => verifyUpdaterFiles({ root, version: '0.1.0', assets: mac.assets }), /plugins\.updater\.pubkey/);
}));
