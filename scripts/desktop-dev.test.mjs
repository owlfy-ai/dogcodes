import assert from 'node:assert/strict';
import { execFileSync, spawn, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';
import { prepareDev } from './desktop-dev.mjs';

const projectRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const identity = 'Developer ID Application: Example (TEAM123)';

test('development selects local certificate settings without importing other local environment values', () => {
  const base = { platform: 'darwin', environment: {},
    localEnvironment: `APPLE_SIGNING_IDENTITY="${identity}"\nPATH=untrusted\nAPPLE_PASSWORD=private`,
    signingConfig: JSON.stringify({ bundle: { macOS: { signingIdentity: 'Fallback' } } }) };
  const plan = prepareDev(base);
  assert.equal(plan.signed, true);
  assert.equal(plan.env.DOGCODES_DEV_SIGNING_IDENTITY, identity);
  assert.equal(plan.env.DOGCODES_DEV_SIGNING_IDENTIFIER, 'codes.dog.desktop');
  assert.equal(plan.env.PATH, undefined);
  assert.equal(plan.env.APPLE_PASSWORD, undefined);
  assert.equal(prepareDev({ ...base, environment: { APPLE_SIGNING_IDENTITY: 'Override' } })
    .env.DOGCODES_DEV_SIGNING_IDENTITY, 'Override');
  assert.equal(prepareDev({ ...base, localEnvironment: '' }).env.DOGCODES_DEV_SIGNING_IDENTITY, 'Fallback');
  assert.throws(() => prepareDev({ ...base, environment: { APPLE_SIGNING_IDENTITY: '-' } }), /ad-hoc/);
});

test('Tauri, Cargo and application arguments retain their boundaries, including paths with spaces', () => {
  const plan = prepareDev({ platform: 'darwin', environment: { APPLE_SIGNING_IDENTITY: identity },
    projectRoot: '/tmp/project with spaces',
    args: ['--no-watch', '--target', 'x86_64-apple-darwin', '--', '--locked', '--', 'hello world', '--flag'] });
  assert.deepEqual(plan.args.slice(0, 5), ['dev', '--no-watch', '--target', 'x86_64-apple-darwin', '--']);
  assert.deepEqual(plan.args.slice(-4), ['--locked', '--', 'hello world', '--flag']);
  for (const target of ['aarch64-apple-darwin', 'x86_64-apple-darwin']) {
    assert.ok(plan.args.includes(`target.${target}.runner = ["/bin/sh","/tmp/project with spaces/scripts/dev-sign-runner.sh"]`));
  }
});

test('unconfigured macOS and other platforms keep normal development arguments', () => {
  const args = ['--no-watch', '--', '--locked', '--', 'argument'];
  for (const platform of ['darwin', 'linux', 'win32']) {
    const environment = platform === 'darwin' ? {} : { APPLE_SIGNING_IDENTITY: identity };
    const plan = prepareDev({ platform, environment, args });
    assert.equal(plan.signed, false);
    assert.deepEqual(plan.args, ['dev', ...args]);
    assert.deepEqual(plan.env, environment);
  }
});

// Private fixture only: never starts DogCodes, reads remote credentials, or
// changes the running development binary. Opt in to using a real certificate.
test('macOS Cargo runner preserves certificate requirements across rebuilds, execs in place, and fails closed', {
  skip: process.platform !== 'darwin' || !process.env.DOGCODES_TEST_SIGNING_IDENTITY,
  timeout: 120_000,
}, async () => {
  const directory = mkdtempSync(join(tmpdir(), 'dogcodes signing fixture '));
  try {
    const manifest = join(directory, 'Cargo.toml');
    const executable = join(directory, 'target/debug/signing-fixture');
    const source = join(directory, 'main.rs');
    writeFileSync(manifest, '[package]\nname="signing-fixture"\nversion="0.0.0"\nedition="2021"\n[[bin]]\nname="signing-fixture"\npath="main.rs"\n[workspace]\n');
    const plan = prepareDev({ platform: 'darwin',
      environment: { ...process.env, APPLE_SIGNING_IDENTITY: process.env.DOGCODES_TEST_SIGNING_IDENTITY },
      identifier: 'codes.dog.desktop.signing-fixture' });
    const cargoConfigs = plan.args.slice(2);
    // Keep build outputs in the private fixture even if the caller has overrides.
    const env = { ...plan.env, CARGO_TARGET_DIR: join(directory, 'target') };
    delete env.CARGO_BUILD_TARGET;
    const build = version => {
      writeFileSync(source, `fn main() { println!("version=${version};pid={};args={:?}", std::process::id(), std::env::args().skip(1).collect::<Vec<_>>()); }\n`);
      const output = execFileSync('cargo', ['run', '--offline', '--quiet', '--manifest-path', manifest,
        ...cargoConfigs, '--', 'hello world', '--flag'], { cwd: directory, env, encoding: 'utf8', timeout: 45_000 });
      assert.match(output, new RegExp(`version=${version};pid=\\d+;args=\\["hello world", "--flag"\\]`));
      const details = spawnSync('/usr/bin/codesign', ['-d', '-r-', executable], { encoding: 'utf8' });
      assert.equal(details.status, 0, details.stderr);
      const requirement = `${details.stdout}\n${details.stderr}`.match(/designated => (.+)/)?.[1];
      assert.ok(requirement, `${details.stdout}\n${details.stderr}`);
      assert.doesNotMatch(requirement, /cdhash/);
      assert.match(requirement, /codes\.dog\.desktop\.signing-fixture/);
      return { requirement, hash: createHash('sha256').update(readFileSync(executable)).digest('hex') };
    };
    const first = build(1);
    const second = build(2);
    assert.notEqual(first.hash, second.hash);
    assert.equal(first.requirement, second.requirement);
    execFileSync('/usr/bin/codesign', ['--verify', '--strict', '-R', `=${first.requirement}`, executable]);

    const runner = join(projectRoot, 'scripts/dev-sign-runner.sh');
    await new Promise((accept, reject) => {
      const child = spawn('/bin/sh', [runner, executable, 'direct argument'], { env, stdio: ['ignore', 'pipe', 'pipe'] });
      let stdout = '', stderr = '';
      child.stdout.on('data', data => { stdout += data; });
      child.stderr.on('data', data => { stderr += data; });
      child.on('error', reject);
      child.on('close', code => {
        try {
          assert.equal(code, 0, stderr);
          assert.ok(stdout.includes(`pid=${child.pid};args=["direct argument"]`), stdout);
          accept();
        } catch (error) { reject(error); }
      });
    });
    const failed = spawnSync('/bin/sh', [runner, executable], { encoding: 'utf8', env: {
      ...env, DOGCODES_DEV_SIGNING_IDENTITY: 'DogCodes nonexistent signing fixture identity',
    }, timeout: 15_000 });
    assert.notEqual(failed.status, 0);
    assert.equal(failed.stdout, '');
  } finally { rmSync(directory, { recursive: true, force: true }); }
});
