import { execFileSync } from 'node:child_process';
import { appendFileSync, readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const scopes = ['frontend', 'native', 'video', 'transport'];
const allChecks = () => Object.fromEntries(scopes.map(scope => [scope, true]));

export function classifyChanges(paths, { full = false } = {}) {
  if (full) return allChecks();
  const checks = Object.fromEntries(scopes.map(scope => [scope, false]));
  for (const path of paths) {
    // Keep embedded Markdown prompts and text fixtures in their code's scope.
    if (path.startsWith('docs/') || /^[^/]+\.(md|mdx|rst)$/i.test(path)
      || /(^|\/)(README(?:\.[^/]+)?|AGENTS\.md)$/i.test(path)
      || /^(LICENSE|NOTICE)(\.|$)/.test(path)) continue;

    // Shared dependencies, CI policy and unknown paths get conservative coverage.
    if (/^(Cargo\.(toml|lock)|rust-toolchain\.toml|package\.json|pnpm-lock\.yaml|pnpm-workspace\.yaml)$/.test(path)
      || /^(\.github\/|\.cargo\/|scripts\/ci-changes)/.test(path)) return allChecks();

    if (path.startsWith('apps/desktop/') && !path.startsWith('apps/desktop/src-tauri/')) {
      checks.frontend = true;
    } else if (path.startsWith('packages/')) {
      Object.assign(checks, allChecks());
    } else if (/^(crates\/|experiments\/|vendor\/|drivers\/|apps\/desktop\/src-tauri\/)/.test(path)) {
      checks.native = true;
      // These shared crates also feed the remote video and transport pipelines.
      if (/^crates\/(remote-desktop|remote-endpoint|remote-transport|protocol|domain|core|runtime|store)\//.test(path)
        || /^vendor\//.test(path)
        || /^apps\/desktop\/src-tauri\/(Cargo\.toml|build\.rs|src\/(daemon(?:\/|\.rs)|lib\.rs))/.test(path)) {
        checks.video = true;
        checks.transport = true;
      }
    } else if (/^scripts\/(relay_qos_trace\.rs|prepare_linux_spa_sdk\.py|test_prepare_linux_spa_sdk\.py|wayland_acceptance\.py|desktop_codec_acceptance\.py)$/.test(path)) {
      checks.native = checks.video = true;
    } else if (path === 'scripts/turn_acceptance.py') {
      checks.native = checks.transport = true;
    } else if (path.startsWith('scripts/fixtures/')) {
      checks.native = checks.video = checks.transport = true;
    } else if (path.startsWith('scripts/') || path.startsWith('release/')) {
      checks.frontend = checks.native = true;
    } else {
      Object.assign(checks, allChecks());
    }
  }
  return checks;
}

export function checksForEvent({ eventName, event, full = false, git = args => execFileSync('git', args, { encoding: 'utf8' }) }) {
  if (full || eventName === 'workflow_dispatch') return allChecks();
  let base;
  let head;
  if (eventName === 'pull_request') {
    base = event.pull_request?.base?.sha;
    head = event.pull_request?.head?.sha;
  } else if (eventName === 'push') {
    base = event.before;
    head = event.after;
  }
  // New branches and missing diff history must never silently skip checks.
  if (![base, head].every(sha => /^[a-f0-9]{40}$/.test(sha ?? '') && !/^0+$/.test(sha))) return allChecks();
  try {
    const range = eventName === 'pull_request' ? `${base}...${head}` : `${base}..${head}`;
    // Disabling rename detection includes both old and new paths in routing.
    const paths = git(['diff', '--name-only', '--no-renames', '-z', range, '--']).split('\0').filter(Boolean);
    return classifyChanges(paths);
  } catch {
    console.warn('Changed files could not be determined; running all checks.');
    return allChecks();
  }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const checks = checksForEvent({
    eventName: process.env.GITHUB_EVENT_NAME,
    event: JSON.parse(readFileSync(process.env.GITHUB_EVENT_PATH, 'utf8')),
    full: process.env.FULL_CHECKS === 'true',
  });
  const summary = Object.entries(checks).map(([scope, enabled]) => `${scope}=${enabled}`).join('\n') + '\n';
  appendFileSync(process.env.GITHUB_OUTPUT, summary);
  console.log(summary);
}
