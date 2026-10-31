import assert from 'node:assert/strict';
import test from 'node:test';
import { checksForEvent, classifyChanges } from './ci-changes.mjs';

const none = { frontend: false, native: false, video: false, transport: false };
const all = { frontend: true, native: true, video: true, transport: true };
const base = 'a'.repeat(40);
const head = 'b'.repeat(40);

test('documentation skips builds, including documentation next to Rust code', () => {
  assert.deepEqual(classifyChanges(['README.md', 'docs/images/example.png', 'crates/core/README.md', 'LICENSE']), none);
});

test('frontend changes avoid all native jobs; mixed changes keep both', () => {
  assert.deepEqual(classifyChanges(['apps/desktop/src/App.tsx', 'apps/desktop/public/startup.css']), { ...none, frontend: true });
  assert.deepEqual(classifyChanges(['apps/desktop/src/App.tsx', 'crates/claude-driver/src/lib.rs']), { ...none, frontend: true, native: true });
});

test('native code and Tauri configuration retain cross-platform tests', () => {
  for (const path of ['crates/zcode-driver/src/lib.rs', 'crates/claude-driver/fixtures/response.txt', 'apps/desktop/src-tauri/capabilities/default.json', 'apps/desktop/src-tauri/tauri.conf.json']) {
    assert.deepEqual(classifyChanges([path]), { ...none, native: true }, path);
  }
});

test('remote and shared native code retain both acceptance suites', () => {
  for (const path of ['crates/remote-desktop/src/lib.rs', 'crates/remote-transport/src/lib.rs', 'crates/core/src/lib.rs', 'crates/core/src/computer_use/workflow.md', 'apps/desktop/src-tauri/src/daemon/remote/wire.rs', 'vendor/webrtc/src/lib.rs']) {
    assert.deepEqual(classifyChanges([path]), { ...none, native: true, video: true, transport: true }, path);
  }
});

test('acceptance harnesses select their matching suite', () => {
  assert.deepEqual(classifyChanges(['scripts/wayland_acceptance.py']), { ...none, native: true, video: true });
  assert.deepEqual(classifyChanges(['scripts/turn_acceptance.py']), { ...none, native: true, transport: true });
});

test('dependency, CI and unknown changes fall back to all checks', () => {
  for (const path of ['Cargo.lock', 'pnpm-lock.yaml', 'package.json', 'packages/protocol/src/index.ts', '.github/workflows/desktop.yml', 'scripts/ci-changes.mjs', 'new-build.config']) {
    assert.deepEqual(classifyChanges([path]), all, path);
  }
});

test('manual and full reusable runs ignore path filters', () => {
  assert.deepEqual(classifyChanges([], { full: true }), all);
  assert.deepEqual(checksForEvent({ eventName: 'workflow_dispatch', event: {} }), all);
  assert.deepEqual(checksForEvent({ eventName: 'push', event: {}, full: true }), all);
});

test('push compares the whole push; PR compares against the merge base', () => {
  for (const eventName of ['push', 'pull_request']) {
    const event = eventName === 'push' ? { before: base, after: head } : { pull_request: { base: { sha: base }, head: { sha: head } } };
    const checks = checksForEvent({ eventName, event, git(args) {
      assert.deepEqual(args, ['diff', '--name-only', '--no-renames', '-z', `${base}${eventName === 'push' ? '..' : '...'}${head}`, '--']);
      return 'apps/desktop/src/App.tsx\0crates/claude-driver/src/lib.rs\0';
    } });
    assert.deepEqual(checks, { ...none, frontend: true, native: true });
  }
});

test('new branches, invalid SHAs and unavailable history fail open', () => {
  for (const before of ['0'.repeat(40), undefined, '--invalid']) {
    assert.deepEqual(checksForEvent({ eventName: 'push', event: { before, after: head }, git() { assert.fail('must not execute git'); } }), all);
  }
  assert.deepEqual(checksForEvent({ eventName: 'push', event: { before: base, after: head }, git() { throw new Error('missing history'); } }), all);
});
