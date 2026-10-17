import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { parseEnv } from 'node:util';
import { validateBlankSigningExample } from './signing-config.mjs';
import { localArtifactPath, localReleasePath } from './source-release-policy.mjs';

const defaultRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
export function privateOrGeneratedPath(path) {
  const lower = path.toLowerCase();
  return localArtifactPath(path) || localReleasePath(path)
    || /(^|\/)(node_modules|dist|target|__pycache__|\.pnpm-store|\.npm|\.cache|\.vite|\.vite-temp|\.turbo|coverage|\.nyc_output|playwright-report|test-results|blob-report|\.venv|venv|\.pytest_cache|\.mypy_cache|\.ruff_cache|\.idea|\.history|\.appledouble)\//.test(lower)
    || /\.(?:py[cod]|log|db(?:-shm|-wal)?|sqlite3?(?:-shm|-wal)?|tsbuildinfo|lcov|profraw|profdata|tmp|bak|swp|swo)$|~$/.test(lower)
    || /\.local(?:\.(?:json|toml|yaml|yml))?$/.test(lower)
    || lower.startsWith('.claude/worktrees/') || lower === '.claude/launch.json'
    || lower.startsWith('.worktrees/') || lower.startsWith('.git-worktrees/')
    || lower.startsWith('apps/desktop/src-tauri/gen/schemas/')
    || /(^|\/)(?:\._[^/]*|\.ds_store|\.lsoverride|thumbs\.db|desktop\.ini)$/.test(lower)
    || /(^|\/)\.env(?:\..*)?$/.test(lower) && !lower.endsWith('/.env.example') && lower !== '.env.example'
    || /\.local\.json$|\.(p12|pfx|p8|pem|key|keychain|keychain-db)$/.test(lower)
    || lower.startsWith('release/private/');
}
function blankEnvironment(text) {
  let variables;
  try { variables = parseEnv(text); } catch { throw new Error('Invalid public environment example format; contents are not shown.'); }
  if (Object.values(variables).some(value => value.trim())) throw new Error('release/.env.example must keep all credential values empty.');
}
export function checkReleaseInputs({ root = defaultRoot, log = console.log } = {}) {
  const git = args => execFileSync('git', args, { cwd: root, encoding: 'utf8', stdio: ['pipe', 'pipe', 'pipe'] });
  const entries = git(['ls-files', '--stage', '-z']).split('\0').filter(Boolean).map(line => {
    const [metadata, path] = [line.slice(0, line.indexOf('\t')), line.slice(line.indexOf('\t') + 1)];
    const [mode, object, stage] = metadata.split(' ');
    return { mode, object, stage, path };
  });
  if (entries.some(entry => entry.stage !== '0')) throw new Error('Resolve Git index conflicts before checking release inputs.');
  const invalid = entries.map(entry => entry.path).filter(privateOrGeneratedPath);
  if (invalid.length) throw new Error(`Remove these private/generated files from the Git index (preserve local copies):\n${invalid.join('\n')}`);
  let indexedExamples = 0;
  for (const [path, validate] of [['release/.env.example', blankEnvironment], ['release/tauri.signing.example.json', validateBlankSigningExample]]) {
    let working;
    try { working = readFileSync(resolve(root, path), 'utf8'); }
    catch { throw new Error(`Cannot read public release example: ${path}`); }
    validate(working);
    const entry = entries.find(entry => entry.path === path);
    if (entry) {
      if (!['100644', '100755'].includes(entry.mode)) throw new Error('Public release examples must be regular files in the Git index.');
      // Check staged bytes too: restoring an empty worktree copy must not hide
      // filled examples already selected for the next commit.
      validate(git(['cat-file', 'blob', entry.object]));
      indexedExamples++;
    }
  }
  const ignore = ['release/.env.local', 'release/tauri.signing.local.json', 'release/private/key.pem', 'certificate.PFX', 'AuthKey.P8'];
  let ignored;
  try {
    ignored = execFileSync('git', ['check-ignore', '--no-index', '--stdin'], {
      cwd: root, input: ignore.join('\n') + '\n', encoding: 'utf8', stdio: ['pipe', 'pipe', 'pipe'],
    }).trim().split('\n');
  } catch { throw new Error('Local signing inputs must be ignored by Git.'); }
  if (!ignore.every(path => ignored.includes(path))) throw new Error('Local signing inputs must be ignored by Git.');
  log(`Release input checks passed: tracked paths checked; ${indexedExamples}/2 examples also checked in the index; worktree examples blank; local signing inputs ignored.`);
  return { trackedFiles: entries.length, indexedExamples };
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try { checkReleaseInputs(); } catch (error) { console.error(error.message); process.exitCode = 1; }
}
