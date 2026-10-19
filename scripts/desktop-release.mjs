import { execFile } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { createReadStream, existsSync, readFileSync, writeFileSync, mkdirSync, mkdtempSync, rmSync, readdirSync, statSync, lstatSync } from 'node:fs';
import { resolve, dirname, basename, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { runBuild } from './signing-macos.mjs';
import { prepareBuild } from './package-desktop.mjs';

const rootDirectory = resolve(dirname(fileURLToPath(import.meta.url)), '..');
export const lockName = 'RELEASE_UPLOAD_LOCK.json';
const startMarker = '<!-- dogcodes-signing:start -->';
const endMarker = '<!-- dogcodes-signing:end -->';
const platformTargets = { darwin: ['aarch64-apple-darwin', 'x86_64-apple-darwin'], win32: ['x86_64-pc-windows-msvc'] };
const markerName = platform => `signing-${platform === 'darwin' ? 'macos' : 'windows'}.json`;
const installer = name => /\.(dmg|exe|deb|AppImage)$/.test(name);
const sleep = ms => new Promise(accept => setTimeout(accept, ms));

export function releaseArguments(args) {
  const options = { check: false, uploadOnly: false };
  for (const arg of args) {
    const key = { '--check': 'check', '--upload-only': 'uploadOnly' }[arg];
    if (!key || options[key]) throw new Error('Usage: pnpm desktop:release [--check | --upload-only]');
    options[key] = true;
  }
  if (options.check && options.uploadOnly) throw new Error('Choose --check or --upload-only.');
  return options;
}

export function releaseVersion(root) {
  const json = path => JSON.parse(readFileSync(resolve(root, path), 'utf8'));
  const version = json('package.json').version;
  const cargo = readFileSync(resolve(root, 'Cargo.toml'), 'utf8').match(/\[workspace\.package\][^\[]*?\nversion\s*=\s*"([^"]+)"/)?.[1];
  if (!/^\d+\.\d+\.\d+(?:-[\da-zA-Z.-]+)?(?:\+[\da-zA-Z.-]+)?$/.test(version)
    || [json('apps/desktop/package.json').version, json('apps/desktop/src-tauri/tauri.conf.json').version, cargo].some(value => value !== version)) {
    throw new Error('The four release version fields must contain the same semantic version.');
  }
  return version;
}

export function githubRepository(remote) {
  const match = remote.trim().match(/^(?:https:\/\/github\.com\/|git@github\.com:|ssh:\/\/git@github\.com\/)([\w.-]+\/[\w.-]+?)(?:\.git)?\/?$/);
  if (!match) throw new Error('origin must point to the intended github.com repository using HTTPS or SSH.');
  return match[1];
}

export async function sha256(file) {
  const hash = createHash('sha256');
  for await (const chunk of createReadStream(file)) hash.update(chunk);
  return hash.digest('hex');
}

function capture(command, args, { cwd, input } = {}) {
  return new Promise((accept, reject) => {
    const env = { ...process.env, GH_PROMPT_DISABLED: '1', GH_HOST: 'github.com' };
    delete env.GH_DEBUG;
    const child = execFile(command, args, { cwd, env, encoding: 'utf8', timeout: 10 * 60000, maxBuffer: 8 * 1024 * 1024 }, (error, stdout, stderr) => {
      if (!error) return accept(stdout.trim());
      const failure = new Error(`${command} ${args[0] ?? ''} failed. Check authentication, connectivity and the command prerequisites.`);
      failure.exitCode = error.code;
      failure.detail = stderr; // Used for classification only; never included in displayed errors.
      reject(failure);
    });
    child.stdin.end(input);
  });
}

function missing(error) { return /HTTP 404|release not found/i.test(error.detail ?? ''); }

export function githubClient({ root, repo, tag, execute = capture }) {
  const gh = args => execute('gh', args, { cwd: root });
  const api = async (path, method, body) => {
    const args = ['api', path];
    if (method) args.push('--method', method);
    if (body) args.push('--input', '-');
    const result = await execute('gh', args, { cwd: root, input: body ? JSON.stringify(body) : undefined });
    return result ? JSON.parse(result) : null;
  };
  return {
    async release() {
      let view;
      try { view = JSON.parse(await gh(['release', 'view', tag, '--repo', repo, '--json', 'databaseId'])); }
      catch (error) { if (missing(error)) return null; throw error; }
      return api(`repos/${repo}/releases/${view.databaseId}`);
    },
    async run(commit) {
      const data = await api(`repos/${repo}/actions/workflows/release.yml/runs?head_sha=${commit}&event=push&per_page=100`);
      return data.workflow_runs.filter(run => run.head_branch === tag).sort((a, b) => b.id - a.id)[0];
    },
    async upload(file, replace = true) {
      await gh(['release', 'upload', tag, file, '--repo', repo, ...(replace ? ['--clobber'] : [])]);
    },
    async download(asset, file) {
      await gh(['release', 'download', tag, '--repo', repo, '--pattern', asset.name, '--output', file]);
    },
    async remove(asset) { await api(`repos/${repo}/releases/assets/${asset.id}`, 'DELETE'); },
    async body(release, body) { await api(`repos/${repo}/releases/${release.id}`, 'PATCH', { body }); },
  };
}

function assertDraft(release) {
  if (!release?.draft) throw new Error('Only an existing draft Release can be updated. Published releases are not replaced.');
}

export async function waitForDraft(client, commit, { wait = sleep, attempts = 600, log = console.log } = {}) {
  let previous;
  for (let attempt = 0; attempt < attempts; attempt++) {
    const release = await client.release();
    if (release) assertDraft(release);
    const run = await client.run(commit);
    if (run?.status === 'completed') {
      if (run.conclusion !== 'success') throw new Error(`Release CI ${run.conclusion}: ${run.html_url}. Re-run the failed CI, then use --upload-only.`);
      if (release) return release;
    }
    if (!run && attempt >= 8) throw new Error('No tag-triggered Release workflow was found. Check Actions permissions and the tag push, then use --upload-only.');
    const state = run ? `GitHub Release CI: ${run.status} (${run.html_url})` : 'Waiting for GitHub to start the tag Release workflow…';
    if (state !== previous) { log(state); previous = state; }
    await wait(15000);
  }
  throw new Error('Timed out waiting for the Release draft. Packages were saved locally; retry with --upload-only.');
}

// Asset names are unique within a Release. Upload without --clobber is the
// cross-machine lock: crashes leave it in place rather than guessing a lease expired.
export async function withReleaseLock(client, stage, work, { wait = sleep, attempts = 120 } = {}) {
  const owner = randomUUID();
  const lockFile = resolve(stage, lockName);
  writeFileSync(lockFile, JSON.stringify({ owner, createdAt: new Date().toISOString() }));
  let held;
  async function belongsToUs(asset) {
    const path = resolve(stage, 'lock-owner.json');
    rmSync(path, { force: true });
    await client.download(asset, path);
    return JSON.parse(readFileSync(path, 'utf8')).owner === owner;
  }
  for (let attempt = 0; attempt < attempts; attempt++) {
    assertDraft(await client.release());
    try { await client.upload(lockFile, false); }
    catch (error) {
      // An upload can have succeeded despite a lost response. Read back ownership.
      const lock = (await client.release())?.assets.find(asset => asset.name === lockName);
      if (!lock) {
        // The previous owner may have released the lock just after our collision.
        if (/already.?exists|already_exists|HTTP 422/i.test(`${error.detail ?? ''} ${error.message}`)) {
          await wait(5000);
          continue;
        }
        throw error;
      }
    }
    const release = await client.release();
    assertDraft(release);
    const lock = release.assets.find(asset => asset.name === lockName);
    if (lock && await belongsToUs(lock)) { held = lock; break; }
    await wait(5000);
  }
  if (!held) throw new Error(`Another computer is updating this draft. Retry later. If it crashed, stop that command before manually removing ${lockName}.`);
  try { return await work(); }
  finally {
    // Delete only our own asset ID, never a successor's lock.
    try { await client.remove(held); }
    catch { console.error(`Could not release ${lockName}. Stop this run before removing that draft attachment manually.`); }
  }
}

function validateAssetName(name) {
  if (name !== basename(name) || !/^[\w.+-]+$/.test(name)) throw new Error('Unsupported release asset filename.');
}

export async function refreshChecksums(client, { tag, commit, stage }) {
  const release = await client.release();
  assertDraft(release);
  const packages = release.assets.filter(asset => installer(asset.name)).sort((a, b) => a.name.localeCompare(b.name, 'en'));
  if (packages.filter(a => a.name.endsWith('.dmg')).length !== 2 || packages.filter(a => a.name.endsWith('.exe')).length !== 1
      || packages.filter(a => a.name.endsWith('.deb')).length !== 1 || packages.filter(a => a.name.endsWith('.AppImage')).length !== 1) {
    throw new Error('The draft must contain exactly two Mac DMGs, one Windows EXE, one DEB and one AppImage.');
  }
  const hashes = new Map();
  for (const asset of packages) {
    validateAssetName(asset.name);
    const digest = asset.digest?.match(/^sha256:([a-f\d]{64})$/i)?.[1]?.toLowerCase();
    if (digest) hashes.set(asset.name, digest);
    else {
      const file = resolve(stage, asset.name);
      await client.download(asset, file);
      hashes.set(asset.name, await sha256(file));
    }
  }
  const complete = {};
  for (const platform of Object.keys(platformTargets)) {
    const marker = release.assets.find(asset => asset.name === markerName(platform));
    complete[platform] = false;
    if (!marker) continue;
    const path = resolve(stage, marker.name);
    await client.download(marker, path);
    const receipt = JSON.parse(readFileSync(path, 'utf8'));
    const expectedNames = packages.filter(asset => asset.name.endsWith(platform === 'darwin' ? '.dmg' : '.exe')).map(asset => asset.name).sort();
    if (receipt.schemaVersion !== 1 || receipt.tag !== tag || receipt.commit !== commit || receipt.platform !== platform
      || !Array.isArray(receipt.assets) || receipt.assets.length !== expectedNames.length
      || receipt.assets.map(asset => asset.name).sort().join('\n') !== expectedNames.join('\n')
      || receipt.assets.some(asset => hashes.get(asset.name) !== asset.sha256 || packages.find(p => p.name === asset.name)?.size !== asset.size)) {
      throw new Error('Signed asset receipt does not match this commit or the actual uploaded files. Retry that platform with --upload-only.');
    }
    complete[platform] = true;
  }
  const checksum = resolve(stage, 'SHA256SUMS.txt');
  writeFileSync(checksum, packages.map(asset => `${hashes.get(asset.name)}  ${asset.name}\n`).join(''));
  const snapshot = release => JSON.stringify(release.assets.filter(asset => installer(asset.name))
    .map(({ id, name, size, digest }) => ({ id, name, size, digest })).sort((a, b) => a.name.localeCompare(b.name, 'en')));
  const unchanged = async () => {
    const current = await client.release();
    assertDraft(current);
    if (snapshot(current) !== snapshot(release)) throw new Error('Release installers changed while checksums were being computed. Retry with --upload-only.');
    return current;
  };
  await unchanged();
  await client.upload(checksum);
  const section = `${startMarker}\n| Platform | Package status |\n| --- | --- |\n| macOS ARM64 / Intel | ${complete.darwin ? 'Developer ID signed and notarized' : 'Awaiting local signed packages'} |\n| Windows x64 | ${complete.win32 ? 'Authenticode signed and timestamped' : 'Awaiting local signed package'} |\n| Linux x64 | CI-built DEB / AppImage; no additional package signature |\n\nSource commit: \`${commit}\`. Verify downloads with \`SHA256SUMS.txt\`.\n${endMarker}`;
  const current = await unchanged();
  const body = current.body ?? '';
  const begin = body.indexOf(startMarker), end = body.indexOf(endMarker);
  const nextBody = begin >= 0 && end >= begin
    ? body.slice(0, begin) + section + body.slice(end + endMarker.length)
    : `${body}\n\n${section}`;
  await client.body(current, nextBody);
  return complete.darwin && complete.win32;
}

export async function uploadSignedPackages(client, receipt, stage, lockOptions) {
  return withReleaseLock(client, stage, async () => {
    for (const asset of receipt.assets) {
      validateAssetName(asset.name);
      assertDraft(await client.release());
      await client.upload(asset.path);
    }
    // Remove differently-named CI variants only after all local packages upload.
    const release = await client.release();
    assertDraft(release);
    const names = new Set(receipt.assets.map(asset => asset.name));
    for (const asset of release.assets) {
      if (asset.name.endsWith(receipt.platform === 'darwin' ? '.dmg' : '.exe') && !names.has(asset.name)) {
        assertDraft(await client.release());
        await client.remove(asset);
      }
    }
    const marker = resolve(stage, markerName(receipt.platform));
    const { schemaVersion, tag, commit, platform } = receipt;
    writeFileSync(marker, JSON.stringify({ schemaVersion, tag, commit, platform,
      assets: receipt.assets.map(({ name, sha256, size }) => ({ name, sha256, size })) }, null, 2));
    assertDraft(await client.release());
    await client.upload(marker);
    return refreshChecksums(client, { tag, commit, stage });
  }, lockOptions);
}

export async function ensureTag({ tag, commit, git, checkOnly = false }) {
  async function remoteCommit() {
    let lines;
    try { lines = await git(['ls-remote', '--exit-code', '--tags', 'origin', `refs/tags/${tag}`, `refs/tags/${tag}^{}`]); }
    catch (error) { if (error.exitCode === 2) return null; throw error; }
    const refs = lines.trim().split('\n').map(line => line.split(/\s+/));
    return (refs.find(([, ref]) => ref.endsWith('^{}')) ?? refs[0])[0];
  }
  const existing = await remoteCommit();
  if (existing && existing !== commit) throw new Error('The remote version tag points to another commit. Bump the version; existing tags are never moved.');
  if (existing) return;
  let local;
  try { local = await git(['rev-parse', '--verify', `refs/tags/${tag}^{commit}`]); }
  catch (error) { if (error.exitCode !== 128) throw error; }
  if (local && local !== commit) throw new Error('The local version tag points to another commit.');
  if (checkOnly) return;
  if (!local) await git(['tag', '-a', tag, '-m', `Release ${tag}`, commit]);
  try { await git(['push', 'origin', `refs/tags/${tag}`]); }
  catch (error) { if (await remoteCommit() !== commit) throw error; }
}

async function verifyPackages({ root, platform, assets }) {
  for (const asset of assets) {
    if (!lstatSync(asset.path).isFile() || lstatSync(asset.path).isSymbolicLink()
      || basename(asset.path) !== asset.name || statSync(asset.path).size !== asset.size || await sha256(asset.path) !== asset.sha256) {
      throw new Error('Local package is missing or has changed. Build it again before uploading.');
    }
    if (platform === 'darwin') {
      await runBuild('codesign', ['--verify', '--strict', asset.path], { cwd: root });
      await runBuild('xcrun', ['stapler', 'validate', asset.path], { cwd: root });
      await runBuild('spctl', ['--assess', '--type', 'open', '--context', 'context:primary-signature', asset.path], { cwd: root });
    } else {
      const settings = prepareBuild({ platform, signed: true, environment: process.env,
        localEnvironment: existsSync(resolve(root, 'release/.env.local')) ? readFileSync(resolve(root, 'release/.env.local'), 'utf8') : '',
        signingConfig: readFileSync(resolve(root, 'release/tauri.signing.local.json'), 'utf8') });
      if (settings.config.bundle.windows.signCommand) throw new Error('Automated release verification currently requires a Windows certificate thumbprint.');
      await runBuild('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', resolve(root, 'scripts/verify-windows-release.ps1'),
        '-Installer', asset.path, '-CertificateThumbprint', settings.config.bundle.windows.certificateThumbprint], { cwd: root });
    }
  }
}

export async function releaseDesktop({ root = rootDirectory, platform = process.platform, args = process.argv.slice(2) } = {}) {
  const options = releaseArguments(args);
  const targets = platformTargets[platform];
  if (!targets) throw new Error('Run this command on the Mac or Windows signing machine. Linux packages come from CI.');
  const git = args => capture('git', args, { cwd: root });
  const version = releaseVersion(root), tag = `v${version}`;
  const commit = await git(['rev-parse', 'HEAD']);
  async function unchanged() {
    if (await git(['rev-parse', 'HEAD']) !== commit || await git(['status', '--porcelain', '--untracked-files=normal'])) {
      throw new Error('Commit the intended source changes first and keep the worktree clean while releasing. Local signing files stay ignored.');
    }
  }
  await unchanged();
  const repo = githubRepository(await git(['remote', 'get-url', 'origin']));
  await capture('gh', ['auth', 'status', '--hostname', 'github.com'], { cwd: root });
  let remote;
  try { remote = await capture('gh', ['api', `repos/${repo}/commits/${commit}`, '--jq', '.sha'], { cwd: root }); }
  catch (error) { if (missing(error)) throw new Error('Push the source commit to GitHub before releasing.'); throw error; }
  if (remote !== commit) throw new Error('Push the source commit to GitHub before releasing.');
  const client = githubClient({ root, repo, tag });
  const existingRelease = await client.release();
  if (existingRelease) assertDraft(existingRelease);
  await ensureTag({ tag, commit, git, checkOnly: true });
  console.log(`Release ${tag}: ${repo} @ ${commit}; ${targets.join(', ')}`);
  const packageScript = resolve(root, 'scripts/package-desktop.mjs');
  if (platform === 'win32') {
    await runBuild('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', resolve(root, 'scripts/verify-windows-release.ps1'), '-CheckTools'], { cwd: root });
  }
  if (options.check) {
    for (const target of targets) await runBuild(process.execPath, [packageScript, '--signed', '--check', '--target', target], { cwd: root });
    console.log('Release preflight passed. No build, tag push or asset change was performed.');
    return;
  }
  const stateDirectory = resolve(root, 'release/private/release-state', tag);
  mkdirSync(stateDirectory, { recursive: true });
  const stateFile = resolve(stateDirectory, `${platform}.json`);
  let receipt;
  if (options.uploadOnly) {
    if (!existsSync(stateFile)) throw new Error('No completed local build receipt exists. Run pnpm desktop:release first.');
    receipt = JSON.parse(readFileSync(stateFile, 'utf8'));
    if (receipt.schemaVersion !== 1 || receipt.commit !== commit || receipt.tag !== tag || receipt.platform !== platform
      || receipt.assets.length !== targets.length) throw new Error('Saved packages belong to another release commit. Rebuild them.');
  } else {
    const pnpm = process.env.npm_execpath;
    if (!pnpm || !/pnpm\.(?:c?js|mjs)$/.test(pnpm)) throw new Error('Run this entry point using pnpm desktop:release.');
    await runBuild(process.execPath, [pnpm, 'install', '--frozen-lockfile'], { cwd: root });
    for (const task of ['test:release', 'check:release-inputs']) await runBuild(process.execPath, [pnpm, task], { cwd: root });
    await runBuild('rustup', ['target', 'add', ...targets], { cwd: root });
    if (platform === 'darwin') {
      const local = readFileSync(resolve(root, 'release/.env.local'), 'utf8');
      const config = readFileSync(resolve(root, 'release/tauri.signing.local.json'), 'utf8');
      if (!prepareBuild({ platform, signed: true, environment: process.env, localEnvironment: local, signingConfig: config }).env.APPLE_NOTARY_PROFILE) {
        throw new Error('Automated Mac release requires APPLE_NOTARY_PROFILE to verify and export the notarized DMGs.');
      }
    }
    const assets = [];
    const cargo = JSON.parse(await capture('cargo', ['metadata', '--format-version', '1', '--no-deps', '--locked'], { cwd: root }));
    for (const target of targets) await runBuild(process.execPath, [packageScript, '--signed', '--check', '--target', target], { cwd: root });
    for (const target of targets) {
      console.log(`Building and signing ${target}…`);
      const started = Date.now();
      await runBuild(process.execPath, [packageScript, '--signed', '--target', target, '--bundles', platform === 'darwin' ? 'dmg' : 'nsis'], { cwd: root });
      const directory = platform === 'darwin' ? resolve(root, 'release/installers', version, target) : join(cargo.target_directory, target, 'release/bundle/nsis');
      const files = readdirSync(directory).filter(name => name.includes(`_${version}_`) && name.endsWith(platform === 'darwin' ? '.dmg' : '.exe')
        && statSync(join(directory, name)).mtimeMs >= started - 2000);
      if (files.length !== 1) throw new Error('Expected exactly one newly built installer for this version and target.');
      const name = files[0], path = join(directory, name);
      assets.push({ name, path, size: statSync(path).size, sha256: await sha256(path) });
    }
    await unchanged();
    receipt = { schemaVersion: 1, tag, commit, platform, assets };
    await verifyPackages({ root, ...receipt });
    writeFileSync(stateFile, JSON.stringify(receipt, null, 2), { mode: 0o600 });
  }
  if (options.uploadOnly) await verifyPackages({ root, ...receipt });
  await unchanged();
  await ensureTag({ tag, commit, git });
  await waitForDraft(client, commit);
  await ensureTag({ tag, commit, git, checkOnly: true });
  await unchanged();
  const stage = mkdtempSync(resolve(stateDirectory, 'upload-'));
  try {
    const ready = await uploadSignedPackages(client, receipt, stage);
    console.log(ready ? 'Both signing platforms are ready; checksums updated. Review installation results and publish the draft manually.' : 'This platform is uploaded and checksums updated. Run the same command on the other signing machine.');
    console.log(`https://github.com/${repo}/releases`);
  } finally { rmSync(stage, { recursive: true, force: true }); }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  releaseDesktop().catch(error => { console.error(error.message); process.exitCode = 1; });
}
