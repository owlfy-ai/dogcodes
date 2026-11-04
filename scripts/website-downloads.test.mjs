import assert from 'node:assert/strict';
import { test } from 'node:test';
import { chooseDownload, signedClientDownloads } from '../website/downloads.mjs';

const url = name => `https://github.com/owlfy-ai/dogcodes/releases/download/v0.1.1/${name}`;
const asset = name => ({ name, browser_download_url: url(name) });

test('signed macOS and Windows installers become direct download links', () => {
  const downloads = signedClientDownloads([
    asset('dog.codes-0.1.1-macOS-AppleSilicon.dmg'),
    asset('dog.codes-0.1.1-macOS-Intel.dmg'),
    asset('dog.codes-0.1.1-Windows-x64-Setup.exe'),
    asset('SHA256SUMS.txt'),
  ]);
  assert.equal(downloads.macArm, url('dog.codes-0.1.1-macOS-AppleSilicon.dmg'));
  assert.equal(downloads.macIntel, url('dog.codes-0.1.1-macOS-Intel.dmg'));
  assert.equal(downloads.windows, url('dog.codes-0.1.1-Windows-x64-Setup.exe'));
  assert.equal(chooseDownload(downloads, { os: 'win' }), downloads.windows);
  assert.equal(chooseDownload(downloads, { os: 'mac', macArch: 'arm' }), downloads.macArm);
  assert.equal(chooseDownload(downloads, { os: 'mac', macArch: 'intel' }), downloads.macIntel);
  assert.equal(chooseDownload(downloads, { os: 'linux' }), null);
});

test('an ambiguous or unsigned asset does not replace a platform link', () => {
  const downloads = signedClientDownloads([
    asset('dog.codes-0.1.1-macOS-AppleSilicon.dmg'),
    { name: 'dog.codes-0.1.1-Windows-x64-Setup.exe' },
    asset('dog.codes-0.1.1-Windows-x64-Setup.exe'),
    asset('other-Windows-x64-Setup.exe'),
  ]);
  assert.equal(downloads.macArm, url('dog.codes-0.1.1-macOS-AppleSilicon.dmg'));
  assert.equal(downloads.macIntel, null);
  assert.equal(downloads.windows, null);
  assert.equal(chooseDownload(downloads, { os: 'mac', macArch: 'intel' }), downloads.macArm);
});
