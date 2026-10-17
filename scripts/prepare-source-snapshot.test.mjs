import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { chmodSync, existsSync, mkdtempSync, mkdirSync, readFileSync, rmSync, statSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';
import { prepareSourceSnapshot } from './prepare-source-snapshot.mjs';

const repository = resolve(dirname(fileURLToPath(import.meta.url)), '..');
function fixture(work) {
  const root = mkdtempSync(resolve(tmpdir(), 'dogcodes-source-snapshot-'));
  const git = args => execFileSync('git', args, { cwd: root, stdio: 'pipe' });
  const put = (path, text) => { mkdirSync(dirname(resolve(root, path)), { recursive: true }); writeFileSync(resolve(root, path), text); };
  try {
    git(['init', '--quiet']);
    put('.gitignore', readFileSync(resolve(repository, '.gitignore')));
    put('release/.env.example', 'APPLE_PASSWORD=\n');
    put('release/tauri.signing.example.json', readFileSync(resolve(repository, 'release/tauri.signing.example.json')));
    put('src/app.ts', 'export const version = 1;\n');
    git(['add', '.gitignore', 'release/.env.example', 'release/tauri.signing.example.json', 'src/app.ts']);
    work({ root, git, put, output: 'release/private/github-source' });
  } finally { rmSync(root, { recursive: true, force: true }); }
}
test('source snapshot includes reviewed working files, no history or ignored credentials, and preserves the original index', () => fixture(({ root, git, put, output }) => {
  put('src/app.ts', 'export const version = 2;\n');
  put('src/needed.ts', 'export const needed = true;\n');
  put('release/.env.local', 'APPLE_PASSWORD=fixture-private-value');
  put('release/private/original-log.txt', 'private fixture');
  const index = git(['diff', '--cached', '--binary']).toString();
  const result = prepareSourceSnapshot({ root, output, reviewedUntracked: ['src/needed.ts'] });
  assert.equal(readFileSync(resolve(result.output, 'src/app.ts'), 'utf8'), 'export const version = 2;\n');
  assert.equal(existsSync(resolve(result.output, 'src/needed.ts')), true);
  assert.equal(existsSync(resolve(result.output, '.git')), false);
  assert.equal(existsSync(resolve(result.output, 'release/.env.local')), false);
  assert.equal(existsSync(resolve(result.output, 'release/private')), false);
  assert.equal(git(['diff', '--cached', '--binary']).toString(), index);
  const manifest = JSON.parse(readFileSync(result.manifestPath));
  assert.equal(manifest.historyIncluded, false);
  assert.equal(manifest.files.length, result.fileCount);
  assert.equal(result.manifestSha256.length, 64);
}));
test('unreviewed source cannot silently enter or disappear from a snapshot', () => fixture(({ root, put, output }) => {
  put('src/unknown.ts', 'unreviewed');
  assert.throws(() => prepareSourceSnapshot({ root, output }), /Review these untracked files/);
  assert.equal(existsSync(resolve(root, output)), false);
}));
test('forced tracked private inputs are rejected before export', () => fixture(({ root, git, put, output }) => {
  put('.env', 'PRIVATE_VALUE=fixture'); git(['add', '-f', '.env']);
  assert.throws(() => prepareSourceSnapshot({ root, output }), /private\/generated/);
  assert.equal(existsSync(resolve(root, output)), false);
}));
test('existing candidates and public output directories are never overwritten', () => fixture(({ root, output, put }) => {
  put(`${output}/keep.txt`, 'keep');
  assert.throws(() => prepareSourceSnapshot({ root, output }), /already exists/);
  assert.equal(readFileSync(resolve(root, output, 'keep.txt'), 'utf8'), 'keep');
  assert.throws(() => prepareSourceSnapshot({ root, output: 'public-source' }), /must be ignored/);
  assert.throws(() => prepareSourceSnapshot({ root, output: root }), /must not contain/);
}));
test('symbolic links cannot import files outside the reviewed source tree', { skip: process.platform === 'win32' }, () => fixture(({ root, output }) => {
  symlinkSync(resolve(root, 'release/.env.example'), resolve(root, 'src/link.ts'));
  assert.throws(() => prepareSourceSnapshot({ root, output, reviewedUntracked: ['src/link.ts'] }), /regular file without symbolic links/);
}));
test('worktree deletions remain deletions and executable source permissions survive', () => fixture(({ root, output, put, git }) => {
  rmSync(resolve(root, 'src/app.ts'));
  put('script.sh', '#!/bin/sh\nexit 0\n');
  if (process.platform !== 'win32') chmodSync(resolve(root, 'script.sh'), 0o755);
  git(['add', 'script.sh']);
  const result = prepareSourceSnapshot({ root, output });
  assert.equal(existsSync(resolve(result.output, 'src/app.ts')), false);
  assert.ok(JSON.parse(readFileSync(result.manifestPath)).deleted.includes('src/app.ts'));
  if (process.platform !== 'win32') assert.ok(statSync(resolve(result.output, 'script.sh')).mode & 0o111);
}));
