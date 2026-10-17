import { spawn } from 'node:child_process';
import { readFileSync, existsSync } from 'node:fs';
import { createRequire } from 'node:module';
import { dirname, isAbsolute, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { parseEnv } from 'node:util';
import { parseSigningConfig } from './signing-config.mjs';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
export const signingVariables = [
  'APPLE_CERTIFICATE', 'APPLE_CERTIFICATE_PASSWORD', 'APPLE_SIGNING_IDENTITY',
  'APPLE_API_ISSUER', 'APPLE_API_KEY', 'APPLE_API_KEY_PATH',
  'APPLE_ID', 'APPLE_PASSWORD', 'APPLE_TEAM_ID',
  'TAURI_SIGNING_PRIVATE_KEY', 'TAURI_SIGNING_PRIVATE_KEY_PASSWORD',
];

// Never echo local file contents or credentials, including in parse errors.
export function prepareBuild({ platform, signed, environment, localEnvironment = '', signingConfig = null }) {
  if (!['darwin', 'win32', 'linux'].includes(platform)) throw new Error('Unsupported desktop build host.');
  const env = { ...environment };
  let config = {
    bundle: {
      macOS: { signingIdentity: null },
      windows: { certificateThumbprint: null, signCommand: null },
    },
  };
  // Do not inherit a hidden Tauri configuration override into a release build.
  delete env.TAURI_CONFIG;
  if (!signed) {
    for (const key of signingVariables) delete env[key];
    // Apple Silicon requires an ad-hoc signature, even for local unsigned builds.
    if (platform === 'darwin') config.bundle.macOS.signingIdentity = '-';
  } else {
    if (platform === 'linux') throw new Error('Linux package signing is not configured by this script.');
    let local;
    try { local = parseEnv(localEnvironment); } catch { throw new Error('Invalid release/.env.local format.'); }
    for (const key of signingVariables) {
      if (!env[key]?.trim() && local[key]?.trim()) env[key] = local[key];
      if (!env[key]?.trim()) delete env[key];
    }
    if (signingConfig) {
      config = parseSigningConfig(signingConfig);
    }
    if (platform === 'darwin') {
      const identity = env.APPLE_SIGNING_IDENTITY || config.bundle?.macOS?.signingIdentity;
      if (identity?.trim() === '-' || (!identity?.trim() && !env.APPLE_CERTIFICATE)) {
        throw new Error('Signed macOS builds require a Developer ID identity or APPLE_CERTIFICATE.');
      }
      const api = ['APPLE_API_ISSUER', 'APPLE_API_KEY', 'APPLE_API_KEY_PATH'];
      const account = ['APPLE_ID', 'APPLE_PASSWORD', 'APPLE_TEAM_ID'];
      const complete = keys => keys.every(key => Boolean(env[key]));
      const present = keys => keys.some(key => Boolean(env[key]));
      if ((present(api) && present(account)) || !(complete(api) || complete(account))) {
        throw new Error('Configure exactly one complete macOS notarization authentication method.');
      }
      if (env.APPLE_API_KEY_PATH && !isAbsolute(env.APPLE_API_KEY_PATH)) {
        throw new Error('APPLE_API_KEY_PATH must be an absolute path.');
      }
    } else {
      const windows = config.bundle?.windows;
      if (!windows?.certificateThumbprint?.trim() && !(typeof windows?.signCommand === 'string' ? windows.signCommand.trim() : windows?.signCommand)) {
        throw new Error('Signed Windows builds require a certificate thumbprint or signing command in the local config.');
      }
      if (windows?.certificateThumbprint?.trim() && !windows.timestampUrl?.trim()) {
        throw new Error('Configure the certificate provider timestamp URL for Windows signing.');
      }
    }
  }
  return { env, config };
}

export function parseArguments(args) {
  let signed = false;
  let mode;
  const forwarded = [];
  for (let i = 0; i < args.length; i++) {
    const arg = args[i];
    if (arg === '--signed' || arg === '--unsigned') {
      if (mode) throw new Error('Choose one signing mode.');
      mode = arg;
      signed = arg === '--signed';
    } else if (['--target', '--bundles'].includes(arg)) {
      const value = args[++i];
      if (!value || value.startsWith('-')) throw new Error(`Missing value for ${arg}.`);
      forwarded.push(arg, value);
    } else {
      throw new Error('Usage: pnpm desktop:package [--unsigned|--signed] [--target TRIPLE] [--bundles FORMAT,...]');
    }
  }
  return { signed, forwarded };
}

function main() {
  const { signed, forwarded } = parseArguments(process.argv.slice(2));
  const envPath = resolve(root, 'release/.env.local');
  const configPath = resolve(root, 'release/tauri.signing.local.json');
  const { env, config } = prepareBuild({
    platform: process.platform, signed, environment: process.env,
    localEnvironment: signed && existsSync(envPath) ? readFileSync(envPath, 'utf8') : '',
    signingConfig: signed && existsSync(configPath) ? readFileSync(configPath, 'utf8') : null,
  });
  const desktop = resolve(root, 'apps/desktop');
  const require = createRequire(resolve(desktop, 'package.json'));
  const cli = resolve(dirname(require.resolve('@tauri-apps/cli/package.json')), 'tauri.js');
  console.log(signed ? 'Building with local signing settings.' : 'Building without distribution signing or notarization.');
  // Use Node directly, avoiding shell quoting and .cmd handling on Windows.
  // Signed settings are passed by filename so secrets in custom commands never enter argv.
  const configArgument = signed && existsSync(configPath) ? configPath : JSON.stringify(config);
  const child = spawn(process.execPath, [cli, 'build', '--config', configArgument, ...forwarded, '--', '--locked'], {
    cwd: desktop, env, stdio: 'inherit', shell: false,
  });
  child.on('error', () => { console.error('Could not launch the Tauri build.'); process.exitCode = 1; });
  child.on('exit', (code, signal) => { process.exitCode = signal ? 1 : (code ?? 1); });
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try { main(); } catch (error) { console.error(error.message); process.exitCode = 1; }
}
