import { mkdtempSync, readdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { runBuild } from './signing-macos.mjs';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');

/** Run the standard suites on the host OS; real-provider and desktop fixtures stay opt-in. */
export async function runLocalTests({ projectRoot = root, environment = process.env, platform = process.platform, run = runBuild } = {}) {
  const pnpm = environment.npm_execpath;
  if (!pnpm || !/pnpm\.(?:c?js|mjs)$/.test(pnpm)) {
    throw new Error('Run local tests and packaging through pnpm (pnpm test:local or pnpm desktop:package).');
  }
  const options = { cwd: projectRoot, env: { ...environment, RUST_MIN_STACK: environment.RUST_MIN_STACK || '16777216' } };
  const scriptTests = readdirSync(resolve(projectRoot, 'scripts')).filter(name => name.endsWith('.test.mjs')).sort()
    .map(name => resolve(projectRoot, 'scripts', name));
  const bridgeTests = readdirSync(resolve(projectRoot, 'crates/zcode-driver/src')).filter(name => name.endsWith('.test.cjs')).sort()
    .map(name => resolve(projectRoot, 'crates/zcode-driver/src', name));

  console.log('Running local script and credential bridge tests…');
  await run(process.execPath, ['--test', ...scriptTests, ...bridgeTests], options);
  await run(platform === 'win32' ? 'python' : 'python3', ['-m', 'unittest', 'discover', '-s', 'scripts', '-p', 'test_*.py', '-v'], options);

  const temporary = mkdtempSync(resolve(tmpdir(), 'dogcodes-local-tests-'));
  try {
    const trace = resolve(temporary, platform === 'win32' ? 'relay-qos-tests.exe' : 'relay-qos-tests');
    await run('rustc', ['--edition=2024', '-D', 'warnings', '--test', 'scripts/relay_qos_trace.rs', '-o', trace], options);
    await run(trace, [], options);
  } finally { rmSync(temporary, { recursive: true, force: true }); }

  console.log('Running local frontend tests…');
  await run(process.execPath, [pnpm, 'test'], options);
  console.log('Running local Rust workspace tests…');
  await run('cargo', ['test', '--workspace', '--locked', '--no-fail-fast', '--', '--nocapture'], options);
  console.log('Local tests passed.');
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  runLocalTests().catch(error => { console.error(error.message); process.exitCode = 1; });
}
