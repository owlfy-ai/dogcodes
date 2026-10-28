import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync, writeFileSync, mkdirSync, existsSync, readdirSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { tmpdir } from 'node:os';
import { resolve } from 'node:path';
import { test } from 'node:test';
import { prepareBuild, parseArguments, signingVariables, linuxAppImageFiles } from './package-desktop.mjs';
import { checkReleaseInputs, privateOrGeneratedPath } from './check-release-inputs.mjs';
import { buildMacKeychainPackage, checkMacKeychain, macPackageOptions } from './signing-macos.mjs';

test('release index guard catches force-added secrets and generated directories', () => {
  for (const path of ['.env', '.env.production', 'release/.env.local', 'any/config.local.json',
    'release/private/token.txt', 'AuthKey.p8', 'secret.pem', 'keys/private.key',
    'apps/desktop/dist/index.html', 'packages/protocol/node_modules/.bin/tsc', 'experiments/__pycache__/probe.pyc']) {
    assert.equal(privateOrGeneratedPath(path), true, path);
  }
  for (const path of ['release/.env.example', '.env.example', 'release/tauri.signing.example.json',
    'crates/core/src/lib.rs', 'README.md']) assert.equal(privateOrGeneratedPath(path), false, path);
});

test('unsigned builds remove inherited signing credentials and config overrides', () => {
  for (const platform of ['win32', 'darwin', 'linux']) {
    const environment = Object.fromEntries(signingVariables.map(key => [key, 'private-value']));
    environment.PATH = 'tool-path';
    environment.TAURI_CONFIG = '{"bundle":{"windows":{"signCommand":"secret"}}}';
    const result = prepareBuild({ platform, signed: false, environment,
      localEnvironment: 'APPLE_PASSWORD=private-value', signingConfig: '{bad JSON' });
    for (const key of signingVariables) assert.equal(result.env[key], undefined);
    assert.equal(result.env.TAURI_CONFIG, undefined);
    assert.equal(result.env.PATH, 'tool-path');
    assert.equal(result.config.bundle.windows.signCommand, null);
    assert.equal(result.config.bundle.macOS.signingIdentity, platform === 'darwin' ? '-' : null);
    assert.equal(environment.APPLE_PASSWORD, 'private-value');
  }
});

test('AppImage pairs the PipeWire client with SDK paths on each native Linux architecture', () => {
  const root = mkdtempSync(resolve(tmpdir(), 'dogcodes appimage '));
  try {
    for (const architecture of ['aarch64-linux-gnu', 'x86_64-linux-gnu', 'lib64']) {
      const libdir = resolve(root, architecture);
      const moduledir = resolve(libdir, 'pipewire-0.3');
      const paths = [resolve(libdir, 'spa-0.2/support/libspa-support.so'),
        resolve(moduledir, 'libpipewire-module-protocol-native.so'),
        resolve(moduledir, 'libpipewire-module-client-node.so')];
      for (const path of paths) {
        mkdirSync(resolve(path, '..'), { recursive: true });
        writeFileSync(path, 'fixture');
      }
      const env = { PKG_CONFIG_PATH: resolve(root, 'sdk path') };
      const files = linuxAppImageFiles({ env, capture(command, args, options) {
        assert.equal(command, 'pkg-config');
        assert.equal(args[1], 'libpipewire-0.3');
        assert.equal(options.env, env);
        return `${args[0] === '--variable=libdir' ? libdir : moduledir}\n`;
      } });
      assert.equal(files['usr/lib/spa-0.2/support/libspa-support.so'], paths[0]);
      assert.equal(files['usr/lib/pipewire-0.3/libpipewire-module-protocol-native.so'], paths[1]);
      assert.equal(files['usr/lib/pipewire-0.3/libpipewire-module-client-node.so'], paths[2]);
      assert.match(readFileSync(files['usr/bin/dogcodes-appimage'], 'utf8'), /SPA_PLUGIN_DIR="\$dogcodes_appdir\/usr\/lib\/spa-0\.2"/);
      assert.match(readFileSync(files['usr/share/applications/dog.codes.desktop'], 'utf8'), /^Exec=dogcodes-appimage$/m);
      assert.match(readFileSync(files['usr/share/dog.codes/pipewire/client.conf'], 'utf8'), /libpipewire-module-client-node/);
      rmSync(paths[2]);
      assert.throws(() => linuxAppImageFiles({ capture: (_, args) => args[0] === '--variable=libdir' ? libdir : moduledir }),
        /Missing AppImage PipeWire client resource:.*client-node/);
    }
    for (const path of ['', 'relative/lib']) {
      assert.throws(() => linuxAppImageFiles({ capture: () => path }), /absolute libdir and moduledir/);
    }
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test('empty templates cannot accidentally produce a signed release', () => {
  const localEnvironment = readFileSync(new URL('../release/.env.example', import.meta.url), 'utf8');
  const signingConfig = readFileSync(new URL('../release/tauri.signing.example.json', import.meta.url), 'utf8');
  for (const platform of ['win32', 'darwin']) {
    assert.throws(() => prepareBuild({ platform, signed: true, environment: {}, localEnvironment, signingConfig }));
  }
});

test('macOS signing requires a real identity and one complete notarization method', () => {
  const environment = { APPLE_SIGNING_IDENTITY: 'Developer ID Application: Example',
    APPLE_ID: 'example@example.invalid', APPLE_PASSWORD: 'private-value', APPLE_TEAM_ID: 'EXAMPLE' };
  const options = { platform: 'darwin', signed: true, environment };
  const result = prepareBuild({ ...options, localEnvironment: 'APPLE_PASSWORD=ignored\nPATH=untrusted' });
  assert.equal(result.env.APPLE_PASSWORD, 'private-value');
  assert.equal(result.env.PATH, undefined);
  assert.throws(() => prepareBuild({ ...options, environment: { ...environment, APPLE_TEAM_ID: '' } }), /complete/);
  assert.throws(() => prepareBuild({ ...options, environment: { ...environment, APPLE_API_KEY: 'EXAMPLE' } }), /exactly one/);
  assert.throws(() => prepareBuild({ ...options, environment: { ...environment, APPLE_SIGNING_IDENTITY: '-' } }), /Developer ID/);
});

test('Keychain notarization accepts an installed identity without exporting credentials', () => {
  const base = { platform: 'darwin', signed: true, environment: {},
    localEnvironment: 'APPLE_NOTARY_PROFILE=ExampleProfile\nAPPLE_SIGNING_IDENTITY="Developer ID Application: Example (TEAM123)"' };
  const result = prepareBuild(base);
  assert.equal(result.config.bundle.macOS.hardenedRuntime, true);
  assert.equal(result.env.APPLE_NOTARY_PROFILE, 'ExampleProfile');
  assert.equal(result.env.APPLE_PASSWORD, undefined);
  for (const key of ['APPLE_ID', 'APPLE_API_KEY', 'APPLE_CERTIFICATE']) {
    assert.throws(() => prepareBuild({ ...base, environment: { [key]: 'private-secret' } }), /must not be combined/);
  }
  assert.throws(() => prepareBuild({ ...base, environment: { APPLE_SIGNING_IDENTITY: 'Apple Development: Example' } }), /full Developer ID/);
  assert.deepEqual(parseArguments(['--signed', '--check']), { signed: true, forwarded: [], check: true });
  assert.throws(() => parseArguments(['--check']), /requires --signed/);
  assert.throws(() => parseArguments(['--signed', '--target', 'one', '--target', 'two']), /only once/);
  assert.throws(() => macPackageOptions({ target: 'x86_64-pc-windows-msvc' }), /macOS Rust target/);
  assert.throws(() => macPackageOptions({ bundles: 'nsis' }), /only app and dmg/);
});

test('Keychain preflight rejects missing identity and missing Rust targets without signing', async () => {
  const base = { root: '.', env: { APPLE_NOTARY_PROFILE: 'ExampleProfile' },
    config: { bundle: { macOS: { signingIdentity: 'Developer ID Application: Example (TEAM123)' } } },
    target: 'universal-apple-darwin' };
  await assert.rejects(checkMacKeychain({ ...base, capture: async () => '' }), /certificate AND private key/);
  const calls = [];
  await assert.rejects(checkMacKeychain({ ...base, capture: async (command, args) => {
    calls.push([command, args]);
    if (command === 'security') return '"Developer ID Application: Example (TEAM123)"';
    if (command === 'rustup') return 'aarch64-apple-darwin\n';
    return '{}';
  } }), /rustup target add x86_64-apple-darwin/);
  assert.equal(calls.some(([, args]) => args.includes('submit')), false);
});

test('Mac artifacts are exported only after app and DMG notarization, stapling and assessment succeed', async () => {
  for (const failure of ['app-rejected', 'dmg-rejected', 'stapler-failed', 'architecture', null]) {
    const root = mkdtempSync(resolve(tmpdir(), 'dogcodes-signing-'));
    try {
      const tauri = resolve(root, 'apps/desktop/src-tauri');
      mkdirSync(tauri, { recursive: true });
      writeFileSync(resolve(tauri, 'tauri.conf.json'), JSON.stringify({ productName: 'dog.codes', identifier: 'codes.dog.desktop', version: '0.1.0' }));
      const calls = [];
      let submissions = 0;
      const parameters = { root, env: { APPLE_NOTARY_PROFILE: 'ExampleProfile' },
        config: { bundle: { macOS: { signingIdentity: 'Developer ID Application: Example (TEAM123)' } } },
        cli: 'tauri.js', configArgument: 'private.json', target: 'aarch64-apple-darwin',
        run: async (command, args, options) => {
          calls.push([command, args]);
          if (command === process.execPath) {
            assert.equal(options.env.APPLE_NOTARY_PROFILE, undefined);
            assert.equal(args[args.indexOf('--bundles') + 1], 'app');
          }
          if (command === 'hdiutil') writeFileSync(args.at(-1), 'signed-package-fixture');
          if (failure === 'stapler-failed' && command === 'xcrun' && args[1] === 'validate') throw new Error('stapler failed');
        },
        capture: async (command, args) => {
          calls.push([command, args]);
          if (command === 'cargo') return JSON.stringify({ target_directory: resolve(root, 'target') });
          if (command === '/usr/libexec/PlistBuddy') return 'desktop';
          if (command === 'lipo') return failure === 'architecture' ? 'x86_64' : 'arm64';
          if (command === 'xcrun' && args[1] === 'submit') {
            submissions++;
            const rejected = failure === 'app-rejected' && submissions === 1 || failure === 'dmg-rejected' && submissions === 2;
            return JSON.stringify({ status: rejected ? 'Invalid' : 'Accepted', id: 'example-id' });
          }
          throw new Error(`Unexpected tool ${command}`);
        } };
      const output = resolve(root, 'release/installers/0.1.0/aarch64-apple-darwin');
      if (failure) {
        await assert.rejects(buildMacKeychainPackage(parameters));
        assert.equal(existsSync(output), false, failure);
      } else {
        await buildMacKeychainPackage(parameters);
        assert.equal(submissions, 2);
        assert.equal(readFileSync(resolve(output, 'dog.codes_0.1.0_aarch64.dmg'), 'utf8'), 'signed-package-fixture');
        assert.match(readFileSync(resolve(output, 'dog.codes_0.1.0_aarch64.dmg.sha256'), 'utf8'), /^[a-f0-9]{64}  dog.codes_/);
        assert.equal(calls.filter(([command]) => command === 'spctl').length, 2);
        assert.equal(calls.filter(([command, args]) => command === 'xcrun' && args[1] === 'validate').length, 2);
      }
      const privateRoot = resolve(root, 'release/private');
      if (existsSync(privateRoot)) assert.deepEqual(readdirSync(privateRoot), []);
    } finally { rmSync(root, { recursive: true, force: true }); }
  }
});

test('Windows signing supports certificate-store or custom-provider configuration', () => {
  const base = { platform: 'win32', signed: true, environment: {} };
  const signingConfig = JSON.stringify({ bundle: { windows: {
    certificateThumbprint: 'EXAMPLE', timestampUrl: 'https://timestamp.example.invalid', digestAlgorithm: 'sha256',
  } } });
  assert.equal(prepareBuild({ ...base, signingConfig }).config.bundle.windows.certificateThumbprint, 'EXAMPLE');
  const custom = JSON.stringify({ bundle: { windows: { signCommand: 'provider-sign %1' } } });
  assert.equal(prepareBuild({ ...base, signingConfig: custom }).config.bundle.windows.signCommand, 'provider-sign %1');
  assert.throws(() => prepareBuild({ ...base, signingConfig: '{"private-secret"' }), error => !error.message.includes('private-secret'));
  assert.throws(() => prepareBuild({ ...base, signingConfig: '{"build":{"beforeBuildCommand":"bad"}}' }), /only.*bundle/);
});

test('CLI rejects configuration and signing overrides outside the local template', () => {
  assert.deepEqual(parseArguments(['--unsigned', '--bundles', 'app']), { signed: false, forwarded: ['--bundles', 'app'] });
  assert.throws(() => parseArguments(['--signed', '--unsigned']));
  assert.throws(() => parseArguments(['--config', 'unsafe.json']));
  assert.throws(() => parseArguments(['--target']));
});

test('Git ignores local credentials and nested outputs but includes blank examples', () => {
  const cwd = mkdtempSync(resolve(tmpdir(), 'dogcodes-release-ignore-'));
  try {
    execFileSync('git', ['init', '--quiet', cwd]);
    const rules = readFileSync(new URL('../.gitignore', import.meta.url));
    // An isolated repository tests the actual rules, regardless of the user's index.
    execFileSync(process.execPath, ['-e', 'require("node:fs").writeFileSync(".gitignore", require("node:fs").readFileSync(0))'], { cwd, input: rules });
    const ignored = ['release/.env.local', 'release/tauri.signing.local.json', 'release/private/key.pem',
      'certificate.p12', 'certificate.pfx', 'AuthKey.p8', 'apps/desktop/node_modules/a', 'apps/desktop/dist/index.html'];
    const result = execFileSync('git', ['check-ignore', '--stdin'], { cwd, input: ignored.join('\n') + '\n', encoding: 'utf8' });
    assert.deepEqual(result.trim().split('\n'), ignored);
    for (const path of ['release/.env.example', 'release/tauri.signing.example.json']) {
      assert.throws(() => execFileSync('git', ['check-ignore', path], { cwd }), error => error.status === 1);
    }
  } finally { rmSync(cwd, { recursive: true, force: true }); }
});

test('local signing input cannot override resources or installer settings or leak parse input', () => {
  const base = { platform: 'win32', signed: true, environment: {} };
  for (const windows of [null, [], { signCommand: ['private-secret'] },
    { signCommand: { cmd: 'sign', args: ['%1'], secret: 'private-secret' } },
    { signCommand: 'sign %1', nsis: { installerHooks: 'private-secret' } }]) {
    assert.throws(() => prepareBuild({ ...base, signingConfig: JSON.stringify({ bundle: { windows } }) }),
      error => !error.message.includes('private-secret'));
  }
  assert.throws(() => prepareBuild({ ...base, signingConfig: JSON.stringify({ bundle: {
    windows: { signCommand: 'sign %1' }, macOS: { files: { key: 'private-secret' } },
  } }) }), /only supported signing/);
  const signCommand = { cmd: 'C:/Program Files/Signer/sign.exe', args: ['--file', '%1'] };
  assert.deepEqual(prepareBuild({ ...base, signingConfig: JSON.stringify({ bundle: { windows: { signCommand } } }) }).config.bundle.windows.signCommand, signCommand);
  assert.throws(() => prepareBuild({ ...base, signingConfig: JSON.stringify({ bundle: { windows: { certificateThumbprint: ' ', timestampUrl: ' ' } } }) }), /require/);
});

function releaseFixture(run) {
  const root = mkdtempSync(resolve(tmpdir(), 'dogcodes-release-index-'));
  try {
    execFileSync('git', ['init', '--quiet', root]);
    mkdirSync(resolve(root, 'release'));
    for (const path of ['.gitignore', 'release/.env.example', 'release/tauri.signing.example.json']) {
      writeFileSync(resolve(root, path), readFileSync(new URL(`../${path}`, import.meta.url)));
    }
    return run(root);
  } finally { rmSync(root, { recursive: true, force: true }); }
}
test('release guard checks staged template contents even after the worktree was restored', () => releaseFixture(root => {
  const envPath = resolve(root, 'release/.env.example');
  const blank = readFileSync(envPath);
  writeFileSync(envPath, 'APPLE_PASSWORD=private-secret');
  execFileSync('git', ['add', 'release/.env.example'], { cwd: root });
  writeFileSync(envPath, blank);
  assert.throws(() => checkReleaseInputs({ root, log() {} }), error => /empty/.test(error.message) && !error.message.includes('private-secret'));
  execFileSync('git', ['add', 'release/.env.example', 'release/tauri.signing.example.json'], { cwd: root });
  assert.equal(checkReleaseInputs({ root, log() {} }).indexedExamples, 2);
}));
test('public signing example rejects hidden fields and parse errors without logging values', () => releaseFixture(root => {
  const path = resolve(root, 'release/tauri.signing.example.json');
  const config = JSON.parse(readFileSync(path));
  config.bundle.macOS.providerShortName = 'private-secret';
  writeFileSync(path, JSON.stringify(config));
  assert.throws(() => checkReleaseInputs({ root, log() {} }), error => !error.message.includes('private-secret'));
  writeFileSync(path, '{"private-secret":');
  assert.throws(() => checkReleaseInputs({ root, log() {} }), error => /Invalid signing JSON/.test(error.message) && !error.message.includes('private-secret'));
}));
test('release guard rejects force-added uppercase private material while preserving local copies', () => releaseFixture(root => {
  writeFileSync(resolve(root, 'Certificate.PFX'), 'private-secret');
  execFileSync('git', ['add', '--force', 'Certificate.PFX'], { cwd: root });
  assert.throws(() => checkReleaseInputs({ root, log() {} }), /Certificate.PFX/);
  assert.equal(readFileSync(resolve(root, 'Certificate.PFX'), 'utf8'), 'private-secret');
}));

test('release policy keeps experiment code and reviewed protocol fixtures, excluding raw captures', () => releaseFixture(root => {
  const local = ['output/review/screen.png', 'output/icon-previews/prompt.md', 'experiments/probe/runs/wire.jsonl',
    'experiments/daemon-acceptance/fixture.json', 'experiments/recovery-analysis/screen.png', 'experiments/probe/results.txt'];
  const publicPaths = ['experiments/acp/src/lib.rs', 'experiments/daemon-acceptance/client.py',
    'experiments/codex-app-server/Cargo.toml', 'experiments/codex-app-server/tests/fixture_export.rs',
    'experiments/acp/fixtures/stable-v1-offline-contract.json',
    'experiments/codex-app-server/fixtures/initialize-handshakes.json',
    'experiments/codex-app-server/fixtures/persistent-resume.json',
    'experiments/codex-app-server/fixtures/schema-fingerprints.json',
    'apps/desktop/src-tauri/icons/icon-master.png', 'docs/assets/overview.png'];
  for (const path of local) assert.equal(privateOrGeneratedPath(path), true, path);
  for (const path of publicPaths) assert.equal(privateOrGeneratedPath(path), false, path);
  const ignored = execFileSync('git', ['check-ignore', '--stdin'], { cwd: root, input: local.join('\n')+'\n', encoding: 'utf8' });
  assert.deepEqual(ignored.trim().split('\n'), local);
  for (const path of publicPaths) assert.throws(() => execFileSync('git', ['check-ignore', path], { cwd: root }), error => error.status === 1);
}));


test('source ignore rules keep required release inputs while excluding local state and outputs', () => releaseFixture(root => {
  const ignored = [
    'release/installers/dogcodes.dmg', 'release/debug-dump.json', 'release/private/signing.pfx',
    '.claude/worktrees/branch/src/lib.rs', '.claude/launch.json', '.worktrees/branch/README.md',
    'apps/desktop/src-tauri/gen/schemas/desktop-schema.json', 'artifacts/home-project-menu.png',
    'experiments/acp/target/debug/probe', '.cache/checks.json', '.pnpm-store/package/index.json',
    'apps/desktop/.vite/deps.json', 'coverage/lcov.info', 'test-results/browser/trace.zip',
    '.venv/bin/python', '.pytest_cache/results', '.idea/workspace.xml',
    'state.sqlite3', 'state.sqlite-wal', 'state.sqlite3-shm', 'profile.profraw',
    'config.local.toml', 'output.tmp', 'backup.bak', 'editor.swp', '._README.md', 'Thumbs.db',
  ];
  const publicPaths = [
    'release/README.md', 'release/.env.example', 'release/tauri.signing.example.json',
    '.github/workflows/desktop.yml', '.claude/commands/review.md', '.claude/skills/review/SKILL.md',
    'apps/desktop/src-tauri/capabilities/default.json', 'apps/desktop/src-tauri/icons/icon-master.png',
    'docs/images/overview.png', 'Cargo.lock', 'pnpm-lock.yaml', 'crates/core/src/lib.rs',
  ];
  for (const path of ignored) assert.equal(privateOrGeneratedPath(path), true, path);
  for (const path of publicPaths) assert.equal(privateOrGeneratedPath(path), false, path);
  const result = execFileSync('git', ['check-ignore', '--no-index', '--stdin'], { cwd: root, input: ignored.join('\n')+'\n', encoding: 'utf8' });
  assert.deepEqual(result.trim().split('\n'), ignored);
  for (const path of publicPaths) assert.throws(() => execFileSync('git', ['check-ignore', '--no-index', path], { cwd: root }), error => error.status === 1);
}));

test('release guard rejects force-added local exports, schemas and captures without deleting them', () => releaseFixture(root => {
  const paths = ['release/session-export.json', 'apps/desktop/src-tauri/gen/schemas/capabilities.json', 'artifacts/acceptance.png', '.claude/launch.json'];
  for (const path of paths) {
    const file = resolve(root, path);
    mkdirSync(resolve(file, '..'), { recursive: true });
    writeFileSync(file, 'local-data');
  }
  execFileSync('git', ['add', '--force', '--', ...paths], { cwd: root });
  assert.throws(() => checkReleaseInputs({ root, log() {} }), error => paths.every(path => error.message.includes(path)) && !error.message.includes('local-data'));
  for (const path of paths) assert.equal(readFileSync(resolve(root, path), 'utf8'), 'local-data');
}));
