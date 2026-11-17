import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { dirname, resolve } from 'node:path';
import { test } from 'node:test';
import { windowsUpdaterInstallers } from './package-desktop.mjs';
import { runBuild } from './signing-macos.mjs';
import { testUpdaterKey } from './fixtures/updater-key.mjs';
import { checkAppArchiveEntries, configuredUpdaterPubkey, signUpdaterPackage, updaterSigningEnvironment, verifyUpdaterSignature,
  withoutUpdaterKey, workspaceVersion } from './updater-signing.mjs';

const repository = resolve(import.meta.dirname, '..');
// Signed by the Tauri CLI with a discarded test key; shared with the Rust updater tests.
const fixtures = resolve(repository, 'apps/desktop/src-tauri/src/app_update/fixtures');
const fixture = name => readFileSync(resolve(fixtures, name));

async function sandbox(work) {
  const root = mkdtempSync(resolve(tmpdir(), 'dogcodes-updater-signing-'));
  const config = pubkey => {
    mkdirSync(resolve(root, 'apps/desktop/src-tauri'), { recursive: true });
    writeFileSync(resolve(root, 'apps/desktop/src-tauri/tauri.conf.json'), JSON.stringify({ plugins: { updater: { pubkey } } }));
  };
  try { return await work({ root, config }); } finally { rmSync(root, { recursive: true, force: true }); }
}

test('the update key reaches only the signer, from exactly one source and with a password', () => sandbox(({ root }) => {
  writeFileSync(resolve(root, 'updater.key'), 'private-value');
  const base = { PATH: 'tool-path', TAURI_SIGNING_PRIVATE_KEY_PASSWORD: 'private-value' };
  assert.deepEqual(withoutUpdaterKey({ ...base, TAURI_SIGNING_PRIVATE_KEY: 'private-value' }), { PATH: 'tool-path' });
  const env = updaterSigningEnvironment({ ...base, TAURI_SIGNING_PRIVATE_KEY_PATH: 'updater.key' }, root);
  assert.equal(env.TAURI_SIGNING_PRIVATE_KEY_PATH, resolve(root, 'updater.key'));
  assert.equal(env.TAURI_SIGNING_PRIVATE_KEY, undefined);
  assert.equal(env.PATH, 'tool-path');
  assert.equal(updaterSigningEnvironment({ ...base, TAURI_SIGNING_PRIVATE_KEY: 'private-value' }, root).TAURI_SIGNING_PRIVATE_KEY, 'private-value');
  assert.throws(() => updaterSigningEnvironment(base, root), /exactly one/);
  assert.throws(() => updaterSigningEnvironment({ ...base, TAURI_SIGNING_PRIVATE_KEY: 'a', TAURI_SIGNING_PRIVATE_KEY_PATH: 'updater.key' }, root), /exactly one/);
  assert.throws(() => updaterSigningEnvironment({ ...base, TAURI_SIGNING_PRIVATE_KEY_PATH: 'missing.key' }, root), /readable key file/);
  assert.throws(() => updaterSigningEnvironment({ TAURI_SIGNING_PRIVATE_KEY_PATH: 'updater.key' }, root), /PASSWORD/);
  for (const error of [() => updaterSigningEnvironment(base, root), () => updaterSigningEnvironment({ TAURI_SIGNING_PRIVATE_KEY: 'private-value' }, root)]) {
    assert.throws(error, problem => !problem.message.includes('private-value'));
  }
}));

test('the committed app configuration carries a well-formed update public key', async () => {
  const pubkey = configuredUpdaterPubkey(repository);
  const key = Buffer.from(Buffer.from(pubkey, 'base64').toString('utf8').split('\n')[1], 'base64');
  assert.equal(key.length, 42);
  assert.equal(key.subarray(0, 2).toString(), 'Ed');
  assert.match(workspaceVersion(repository), /^\d+\.\d+\.\d+/);
  await sandbox(({ root, config }) => { config(''); assert.throws(() => configuredUpdaterPubkey(root), /plugins\.updater\.pubkey/); });
});

test('signatures are checked like the app does: key, package bytes, signed comment and version', () => {
  const pubkey = fixture('test-only.pub').toString();
  const signature = fixture('package.bin.sig').toString();
  assert.equal(verifyUpdaterSignature({ bytes: fixture('package.bin'), signature, pubkey }), '0.2.0');
  assert.equal(verifyUpdaterSignature({ bytes: fixture('package.bin'), signature: fixture('package-unversioned.bin.sig').toString(), pubkey }), null);
  assert.throws(() => verifyUpdaterSignature({ bytes: Buffer.from('other'), signature, pubkey }), /does not match the package/);
  assert.throws(() => verifyUpdaterSignature({ bytes: fixture('package.bin'), signature, pubkey: testUpdaterKey().pubkey }), /different key/);
  const forged = Buffer.from(Buffer.from(signature, 'base64').toString('utf8').replace('version:0.2.0', 'version:9.9.9')).toString('base64');
  assert.throws(() => verifyUpdaterSignature({ bytes: fixture('package.bin'), signature: forged, pubkey }), /comment is not signed/);
  assert.throws(() => verifyUpdaterSignature({ bytes: Buffer.from(''), signature: 'bm90IGEgc2lnbmF0dXJl', pubkey }), /Malformed/);
  const key = testUpdaterKey();
  assert.equal(verifyUpdaterSignature({ bytes: Buffer.from('x'), signature: key.signature(Buffer.from('x'), '1.2.3'), pubkey: key.pubkey }), '1.2.3');
});

test('signing passes the key through the environment and keeps only a signature bound to this version', async () => sandbox(async ({ root, config }) => {
  const key = testUpdaterKey();
  config(key.pubkey);
  const file = resolve(root, 'package.exe');
  writeFileSync(file, 'installer');
  const env = { TAURI_SIGNING_PRIVATE_KEY: 'private-value', TAURI_SIGNING_PRIVATE_KEY_PASSWORD: 'private-value' };
  const calls = [];
  const run = signedVersion => async (command, args, options) => {
    calls.push({ args, options });
    writeFileSync(`${args.at(-1)}.sig`, key.signature(readFileSync(args.at(-1)), signedVersion));
  };
  assert.equal(await signUpdaterPackage({ root, cli: 'tauri.js', file, version: '0.2.0', env, run: run('0.2.0') }), `${file}.sig`);
  assert.deepEqual(calls[0].args, ['tauri.js', 'signer', 'sign', '--app-version', '0.2.0', file]);
  assert.equal(calls[0].args.some(arg => arg.includes('private-value')), false);
  assert.equal(calls[0].options.env.TAURI_SIGNING_PRIVATE_KEY, 'private-value');
  await assert.rejects(signUpdaterPackage({ root, cli: 'tauri.js', file, version: '0.2.0', env, run: run(null) }), /bound to no version/);
  rmSync(`${file}.sig`);
  await assert.rejects(signUpdaterPackage({ root, cli: 'tauri.js', file, version: '0.2.0', env, run: async () => {} }), /wrote no signature/);
  config(testUpdaterKey().pubkey);
  await assert.rejects(signUpdaterPackage({ root, cli: 'tauri.js', file, version: '0.2.0', env, run: run('0.2.0') }), /different key/);
}));

test('the pinned Tauri CLI signs with a password-protected key file and binds the version', async () => sandbox(async ({ root }) => {
  const cli = resolve(dirname(createRequire(resolve(repository, 'apps/desktop/package.json')).resolve('@tauri-apps/cli/package.json')), 'tauri.js');
  const keyFile = resolve(root, 'private/updater.key');
  mkdirSync(dirname(keyFile));
  execFileSync(process.execPath, [cli, 'signer', 'generate', '--ci', '--password', 'test-password', '--write-keys', keyFile], { stdio: 'pipe' });
  mkdirSync(resolve(root, 'apps/desktop/src-tauri'), { recursive: true });
  writeFileSync(resolve(root, 'apps/desktop/src-tauri/tauri.conf.json'), JSON.stringify({ plugins: { updater: { pubkey: readFileSync(`${keyFile}.pub`, 'utf8') } } }));
  const file = resolve(root, 'dog.codes-0.2.0-Windows-x64-Setup.exe');
  writeFileSync(file, 'installer');
  const env = { ...withoutUpdaterKey(process.env), TAURI_SIGNING_PRIVATE_KEY_PATH: keyFile, TAURI_SIGNING_PRIVATE_KEY_PASSWORD: 'test-password' };
  const quiet = runBuild;
  await signUpdaterPackage({ root, cli, file, version: '0.2.0', env, run: quiet });
  assert.equal(existsSync(`${file}.sig`), true);
  await assert.rejects(signUpdaterPackage({ root, cli, file, version: '0.2.0', run: quiet,
    env: { ...env, TAURI_SIGNING_PRIVATE_KEY_PASSWORD: 'wrong-password' } }));
}));

test('a macOS update archive holds only the app bundle', () => {
  checkAppArchiveEntries('dog.codes.app/\ndog.codes.app/Contents/\ndog.codes.app/Contents/Info.plist\n', 'dog.codes.app');
  checkAppArchiveEntries('./dog.codes.app/Contents/Info.plist\n', 'dog.codes.app');
  assert.throws(() => checkAppArchiveEntries('', 'dog.codes.app'), /only dog\.codes\.app/);
  for (const listing of ['._dog.codes.app\n', 'dog.codes.app/Contents/._Info.plist\n', 'other.app/Contents/Info.plist\n', 'dog.codes.application/x\n']) {
    assert.throws(() => checkAppArchiveEntries(`dog.codes.app/\n${listing}`, 'dog.codes.app'), /only dog\.codes\.app/, listing);
  }
});

test('Windows signs the NSIS installers of this version only', () => sandbox(({ root }) => {
  for (const name of ['dog.codes-0.2.0-Windows-x64-Setup.exe', 'dog.codes-0.2.0-Windows-x64-Setup.exe.sig', 'dog.codes-0.1.9-Windows-x64-Setup.exe', 'dog.codes_0.2.0_x64-setup.exe']) {
    writeFileSync(resolve(root, name), '');
  }
  assert.deepEqual(windowsUpdaterInstallers(root, '0.2.0'), ['dog.codes-0.2.0-Windows-x64-Setup.exe']);
  assert.deepEqual(windowsUpdaterInstallers(resolve(root, 'missing'), '0.2.0'), []);
}));
