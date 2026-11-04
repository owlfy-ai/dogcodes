import assert from 'node:assert/strict';
import { test } from 'node:test';
import { hostTarget, linkerVariable, shimTargets } from './prepare-shims.mjs';

test('every host builds the Linux shims and only macOS builds the Apple ones', () => {
  const linux = ['x86_64-unknown-linux-musl', 'aarch64-unknown-linux-musl'];
  assert.deepEqual(shimTargets({ platform: 'linux' }), linux);
  assert.deepEqual(shimTargets({ platform: 'win32' }), linux);
  assert.deepEqual(shimTargets({ platform: 'darwin' }), [...linux, 'x86_64-apple-darwin', 'aarch64-apple-darwin']);
});

test('development builds the shim for this machine', () => {
  assert.equal(hostTarget({ platform: 'darwin', arch: 'arm64' }), 'aarch64-apple-darwin');
  assert.equal(hostTarget({ platform: 'linux', arch: 'x64' }), 'x86_64-unknown-linux-musl');
  assert.equal(hostTarget({ platform: 'win32', arch: 'x64' }), null);
});

test('the linker variable follows the naming cargo reads', () => {
  assert.equal(linkerVariable('x86_64-unknown-linux-musl'), 'CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER');
});
