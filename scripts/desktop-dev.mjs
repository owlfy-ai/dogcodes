import { spawn } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { parseEnv } from 'node:util';
import { parseSigningConfig } from './signing-config.mjs';
import { prepareComputerUse } from './prepare-computer-use.mjs';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');

export function prepareDev({ platform, environment, args = [], localEnvironment = '', signingConfig = null,
  projectRoot = root, identifier = 'codes.dog.desktop' }) {
  const env = { ...environment };
  if (platform !== 'darwin') return { env, args: ['dev', ...args], signed: false };
  let local;
  try { local = parseEnv(localEnvironment); }
  catch { throw new Error('Invalid release/.env.local format.'); }
  const config = signingConfig ? parseSigningConfig(signingConfig) : null;
  const identity = [env.APPLE_SIGNING_IDENTITY, local.APPLE_SIGNING_IDENTITY,
    config?.bundle.macOS?.signingIdentity].find(value => value?.trim())?.trim();
  if (!identity) return { env, args: ['dev', ...args], signed: false };
  if (identity === '-') throw new Error('Development Keychain access needs a certificate identity, not ad-hoc signing.');

  env.DOGCODES_DEV_SIGNING_IDENTITY = identity;
  env.DOGCODES_DEV_SIGNING_IDENTIFIER = identifier;
  // Tauri dev uses `cargo run`. A Cargo target runner signs after every build,
  // including hot reload, then execs the binary so process ownership is preserved.
  // TOML arrays preserve paths containing spaces without invoking a shell parser.
  const runner = JSON.stringify(['/bin/sh', resolve(projectRoot, 'scripts/dev-sign-runner.sh')]);
  const cargoConfig = ['aarch64-apple-darwin', 'x86_64-apple-darwin'].flatMap(target =>
    ['--config', `target.${target}.runner = ${runner}`]);
  const separator = args.indexOf('--');
  const tauriArgs = separator < 0 ? args : args.slice(0, separator);
  const cargoArgs = separator < 0 ? [] : args.slice(separator + 1);
  return { env, args: ['dev', ...tauriArgs, '--', ...cargoConfig, ...cargoArgs], signed: true };
}

async function main() {
  const readLocal = path => process.platform === 'darwin' && existsSync(path) ? readFileSync(path, 'utf8') : '';
  const appConfig = JSON.parse(readFileSync(resolve(root, 'apps/desktop/src-tauri/tauri.conf.json'), 'utf8'));
  const plan = prepareDev({ platform: process.platform, environment: process.env, args: process.argv.slice(2),
    localEnvironment: readLocal(resolve(root, 'release/.env.local')),
    signingConfig: readLocal(resolve(root, 'release/tauri.signing.local.json')) || null,
    identifier: appConfig.identifier });
  const targetIndex = plan.args.indexOf('--target');
  const target = targetIndex >= 0 ? plan.args[targetIndex + 1] : undefined;
  await prepareComputerUse({ target, signingIdentity: plan.env.DOGCODES_DEV_SIGNING_IDENTITY || '-' });
  if (process.platform === 'darwin') console.log(plan.signed
    ? `Development signing: ${plan.env.DOGCODES_DEV_SIGNING_IDENTITY} (${appConfig.identifier}).`
    : 'Development signing: ad-hoc. Set APPLE_SIGNING_IDENTITY in release/.env.local for stable Keychain access.');
  const desktop = resolve(root, 'apps/desktop');
  const require = createRequire(resolve(desktop, 'package.json'));
  const cli = resolve(dirname(require.resolve('@tauri-apps/cli/package.json')), 'tauri.js');
  const child = spawn(process.execPath, [cli, ...plan.args], { cwd: desktop, env: plan.env, stdio: 'inherit' });
  const interrupt = () => child.kill('SIGINT');
  const terminate = () => child.kill('SIGTERM');
  process.on('SIGINT', interrupt);
  process.on('SIGTERM', terminate);
  child.on('error', error => { console.error(error.message); process.exitCode = 1; });
  child.on('exit', (code, signal) => {
    process.removeListener('SIGINT', interrupt);
    process.removeListener('SIGTERM', terminate);
    if (signal) process.kill(process.pid, signal);
    else process.exitCode = code ?? 1;
  });
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch(error => { console.error(error.message); process.exitCode = 1; });
}
