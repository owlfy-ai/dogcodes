import { spawn } from 'node:child_process';
import { mkdtempSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
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
  // MSVC link.exe runs out of memory when many debug binaries link at once.
  const cargoEnv = platform === 'win32' && !options.env.CARGO_BUILD_JOBS
    ? { ...options.env, CARGO_BUILD_JOBS: '2' }
    : options.env;
  const cargo = ['test', '--workspace', '--locked', '--no-fail-fast', '--', '--nocapture'];
  if (run === runBuild) await runCargoTests(cargo, { ...options, env: cargoEnv });
  else await run('cargo', cargo, { ...options, env: cargoEnv });
  console.log('Local tests passed.');
}

/** Keep every failing Rust test visible after later packages have filled the terminal. */
export function rustFailureReport(output) {
  const lines = output.split(/\r?\n/);
  const panics = new Map();
  const groups = [];
  let current = [];
  const takePanic = index => {
    const header = lines[index].match(/^thread '([^']+)'(?: \(\d+\))? panicked at /);
    if (!header) return;
    const body = [lines[index]];
    for (let next = index + 1; next < lines.length; next++) {
      if (/^(?:thread '|test |note: run with )/.test(lines[next])) break;
      body.push(lines[next]);
    }
    panics.set(header[1], body);
  };
  for (let index = 0; index < lines.length; index++) {
    takePanic(index);
    const result = lines[index].match(/^test (.+) \.\.\. (ok|FAILED)$/);
    if (result && result[2] === 'ok') panics.delete(result[1]);
    if (result && result[2] === 'FAILED') current.push({ name: result[1], detail: panics.get(result[1]) });
    const rerun = lines[index].match(/^error: test failed, to rerun pass (.+)$/);
    if (rerun) {
      groups.push({ target: rerun[1], tests: current });
      current = [];
    }
  }
  if (current.length) groups.push({ target: '', tests: current });
  const targets = [];
  for (let index = 0; index < lines.length; index++) {
    if (!/^error: \d+ targets? failed:/.test(lines[index])) continue;
    for (let next = index + 1; next < lines.length && /^\s+`-p /.test(lines[next]); next++) targets.push(lines[next].trim());
  }
  if (!groups.length && !targets.length) return '';
  const rendered = ['Failing Rust tests:'];
  for (const group of groups) {
    if (group.target) rendered.push(group.target);
    for (const failure of group.tests) {
      rendered.push(`  ${failure.name}`);
      if (failure.detail) for (const line of failure.detail) rendered.push(`    ${line}`);
    }
  }
  if (targets.length) rendered.push(targets.join('\n'));
  return rendered.join('\n');
}

function runCargoTests(args, options) {
  const log = join(tmpdir(), `dogcodes-cargo-test-${process.pid}.log`);
  return new Promise((accept, reject) => {
    const child = spawn('cargo', args, { ...options, stdio: ['inherit', 'pipe', 'pipe'], shell: false });
    const chunks = [];
    for (const [stream, destination] of [[child.stdout, process.stdout], [child.stderr, process.stderr]]) {
      stream.on('data', chunk => { chunks.push(chunk); destination.write(chunk); });
    }
    child.on('error', () => reject(new Error('Could not launch cargo.')));
    child.on('close', (code, signal) => {
      const output = Buffer.concat(chunks).toString('utf8');
      if (code === 0 && !signal) { rmSync(log, { force: true }); return accept(); }
      writeFileSync(log, output);
      const report = rustFailureReport(output);
      console.error(report
        ? `\n${report}\n\nFull Rust test output: ${log}`
        : `\nRust test output did not include a failure summary.\nFull Rust test output: ${log}`);
      reject(new Error('cargo failed; packaging stopped.'));
    });
  });
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  runLocalTests().catch(error => { console.error(error.message); process.exitCode = 1; });
}
