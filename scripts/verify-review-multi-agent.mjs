// Read-only handoff check. Does not start agents, commit, stage, or alter files.
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync, readdirSync } from 'node:fs';
import { join, resolve } from 'node:path';

const [directory, phase] = process.argv.slice(2);
if (!directory || !['mixed', 'history'].includes(phase)) {
  console.error('Usage: node scripts/verify-review-multi-agent.mjs <fixture-directory> <mixed|history>');
  process.exit(2);
}
const root = resolve(directory);
const read = path => readFileSync(join(root, path), 'utf8');
assert.equal(read('acceptance-seeded'), 'dogcodes isolated review fixture v1\n');
const mixed = JSON.parse(read('acceptance-report.json'));
assert.equal(mixed.passed, true, 'mixed-agent run did not pass');
assert.equal(mixed.error, null);
const expected = [
  'mixed-git: codex turn 1; original baseline to actual final file',
  'mixed-git: claude-code turn 2; original baseline to actual final file',
  'mixed-git: codex turn 3; original baseline to actual final file',
  'mixed Git: HEAD/index/working comparisons remain distinct after agent switches',
  'mixed-non-git: claude-code turn 1; original baseline to actual final file',
  'mixed-non-git: codex turn 2; original baseline to actual final file',
  'mixed-non-git: claude-code turn 3; original baseline to actual final file',
];
assert.deepEqual(mixed.checks, expected);
for (const project of ['live-project', 'live-git-project']) {
  assert.equal(read(`${project}/sample.txt`), 'mixed edit 3\n', `${project}: final file differs`);
}
const env = Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith('GIT_')));
env.GIT_CONFIG_NOSYSTEM = '1';
env.GIT_CONFIG_GLOBAL = process.platform === 'win32' ? 'NUL' : '/dev/null';
const git = (...args) => execFileSync('git', ['--no-optional-locks', '-C', join(root, 'live-git-project'), ...args], { env, encoding: 'utf8' });
assert.equal(git('show', ':sample.txt'), 'mixed edit 1\n', 'index differs from first real edit');
assert.equal(existsSync(join(root, 'live-git-project/.git/index.lock')), false);
if (phase === 'mixed') {
  assert.equal(git('show', 'HEAD:sample.txt'), 'before real model edit\n');
  assert.equal(git('diff', '--cached', '--name-only'), 'sample.txt\n');
} else {
  const names = readdirSync(root).filter(name => /^history-report-\d+\.json$/.test(name)).sort();
  assert(names.length, 'history report missing');
  const latest = JSON.parse(read(names.at(-1)));
  assert.equal(latest.passed, true, 'latest history run did not pass');
  assert.equal(latest.error, null);
  assert.deepEqual(latest.checks, [
    'committed: exact real-model content after process restart',
    'committed: exact real-model content after process restart',
    'branch: exact real-model content after process restart',
    'uncommitted: exact real-model content after process restart',
    'unstaged: exact real-model content after process restart',
    'staged: empty after committing first model edit',
    'non-Git: original baseline survives full desktop process restart after agent switches',
  ]);
  assert.equal(git('show', 'HEAD:sample.txt'), 'mixed edit 1\n');
  assert.equal(git('show', 'HEAD^:sample.txt'), 'before real model edit\n');
  assert.equal(git('rev-parse', 'review-acceptance-base'), git('rev-parse', 'HEAD^'));
  assert.equal(git('diff', '--cached', '--name-only'), '');
  console.log(`History report: ${names.at(-1)}`);
}
console.log(`PASS (${phase}): saved reports, both final files, HEAD/index and lock state agree. No files changed.`);
