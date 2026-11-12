import assert from 'node:assert/strict';
import { existsSync } from 'node:fs';
import { test } from 'node:test';
import { runLocalTests } from './test-local.mjs';
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
  }
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
