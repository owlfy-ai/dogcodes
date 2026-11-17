import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { test } from 'node:test';
import { runLocalTests, rustFailureReport } from './test-local.mjs';
import { parseArguments } from './package-desktop.mjs';

test('local packaging tests by default and consumes the CI skip option without forwarding it to Tauri', () => {
  assert.equal(parseArguments(['--unsigned']).skipTests, undefined);
  assert.deepEqual(parseArguments(['--unsigned', '--skip-tests', '--target', 'x86_64-pc-windows-msvc']), {
    signed: false, skipTests: true, forwarded: ['--target', 'x86_64-pc-windows-msvc'],
  });
  assert.throws(() => parseArguments(['--skip-tests', '--skip-tests']), /Usage/);
});

test('local suites include script, bridge, Python, relay, frontend and Rust tests on each host', async () => {
  for (const platform of ['darwin', 'win32', 'linux']) {
    const calls = [];
    await runLocalTests({ platform, environment: { npm_execpath: '/tools/pnpm.cjs' },
      run: async (command, args, options) => { calls.push({ command, args, options }); } });
    const [scripts, python, compile, trace, frontend, native] = calls;
    assert.equal(calls.length, 6);
    assert.equal(scripts.command, process.execPath);
    assert.equal(scripts.args[0], '--test');
    assert.ok(scripts.args.some(path => path.endsWith('prepare-openssh.test.mjs')));
    assert.ok(scripts.args.some(path => path.endsWith('test-local.test.mjs')));
    assert.ok(scripts.args.some(path => path.endsWith('account-reader.test.cjs')));
    assert.equal(python.command, platform === 'win32' ? 'python' : 'python3');
    assert.deepEqual(python.args, ['-m', 'unittest', 'discover', '-s', 'scripts', '-p', 'test_*.py', '-v']);
    assert.equal(compile.command, 'rustc');
    assert.equal(compile.args.at(-1), trace.command);
    assert.equal(trace.command.endsWith('.exe'), platform === 'win32');
    assert.equal(existsSync(trace.command), false);
    assert.deepEqual(frontend.args, ['/tools/pnpm.cjs', 'test']);
    assert.equal(native.command, 'cargo');
    assert.deepEqual(native.args, ['test', '--workspace', '--locked', '--no-fail-fast', '--', '--nocapture']);
    assert.equal(native.options.env.RUST_MIN_STACK, '16777216');
    assert.equal(native.options.env.CARGO_BUILD_JOBS, platform === 'win32' ? '2' : undefined);
  }
});

test('Rust failure report keeps real assertions and skips deliberate panics', () => {
  const report = rustFailureReport(`
thread '<unnamed>' (1) panicked at crates/remote-desktop/src/video/worker.rs:658:13:
deliberate fixture failure
test video::worker::tests::a_panicking_fixture_does_not_fail_the_tests_after_it ... ok
thread 'environment::shell_tests::private_login_shell_supplies_missing_auth_without_overriding_explicit_credentials' (2) panicked at crates/claude-driver/src/environment.rs:254:22:
called \`Option::unwrap()\` on a \`None\` value
note: run with \`RUST_BACKTRACE=1\` environment variable to display a backtrace
test environment::shell_tests::private_login_shell_supplies_missing_auth_without_overriding_explicit_credentials ... FAILED
error: test failed, to rerun pass \`-p claude-driver --lib\`
test other::case ... FAILED
error: test failed, to rerun pass \`-p antigravity-driver --lib\`
error: 2 targets failed:
    \`-p antigravity-driver --lib\`
    \`-p claude-driver --lib\`
`);
  assert.match(report, /private_login_shell_supplies_missing_auth_without_overriding_explicit_credentials/);
  assert.match(report, /called `Option::unwrap\(\)` on a `None` value/);
  assert.match(report, /other::case/);
  assert.match(report, /`-p claude-driver --lib`/);
  assert.match(report, /`-p antigravity-driver --lib`/);
  assert.doesNotMatch(report, /deliberate fixture failure/);
});

test('a failed suite stops the sequence and removes temporary test binaries', async () => {
  let trace;
  const commands = [];
  const failure = new Error('relay test failed');
  await assert.rejects(runLocalTests({ environment: { npm_execpath: '/tools/pnpm.cjs' }, run: async (command, args) => {
    commands.push(command);
    if (command === 'rustc') trace = args.at(-1);
    if (command === trace) throw failure;
  } }), error => error === failure);
  assert.equal(commands.length, 4);
  assert.equal(existsSync(trace), false);
});
