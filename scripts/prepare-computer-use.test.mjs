import { test } from 'node:test';
import assert from 'node:assert/strict';
import { computerUseDistribution } from './prepare-computer-use.mjs';

test('computer use selects target binaries rather than the build host architecture', () => {
  const linux = computerUseDistribution({platform:'win32',arch:'x64',target:'aarch64-unknown-linux-gnu'});
  assert.equal(linux.platform, 'linux');
  assert.equal(linux.arch, 'arm64');
  assert.match(linux.archive, /linux-arm64-binary/);
  const mac = computerUseDistribution({platform:'darwin',arch:'arm64',target:'x86_64-apple-darwin'});
  assert.equal(mac.arch,'universal');
  assert.match(mac.archive,/darwin-universal-binary/);
  assert.match(mac.digest,/^[a-f0-9]{64}$/);
  assert.deepEqual(computerUseDistribution({platform:'win32',arch:'x64'}).binaries,
    ['cua-driver.exe','cua-driver-uia.exe','cua-cursor-theme.exe']);
});
test('unpublished targets fail explicitly instead of shipping a host binary', () => {
  assert.throws(()=>computerUseDistribution({target:'x86_64-unknown-linux-musl'}),/Unsupported/);
  assert.throws(()=>computerUseDistribution({platform:'linux',arch:'riscv64'}),/Unsupported/);
});
