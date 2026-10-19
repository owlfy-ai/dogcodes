import { readFileSync, existsSync, mkdirSync, mkdtempSync, writeFileSync, rmSync } from 'node:fs';
import { createRequire } from 'node:module';
import { dirname, isAbsolute, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { parseEnv } from 'node:util';
import { parseSigningConfig } from './signing-config.mjs';
import { checkMacKeychain, buildMacKeychainPackage, runBuild } from './signing-macos.mjs';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
export const signingVariables = [
  'APPLE_CERTIFICATE', 'APPLE_CERTIFICATE_PASSWORD', 'APPLE_SIGNING_IDENTITY',
  'APPLE_API_ISSUER', 'APPLE_API_KEY', 'APPLE_API_KEY_PATH',
  'APPLE_ID', 'APPLE_PASSWORD', 'APPLE_TEAM_ID',
  'APPLE_NOTARY_PROFILE',
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
      const profile = env.APPLE_NOTARY_PROFILE;
      if (profile) {
        if (present(api) || present(account) || env.APPLE_CERTIFICATE || env.APPLE_CERTIFICATE_PASSWORD) {
          throw new Error('Keychain notarization must not be combined with Apple API/account credentials or certificate imports.');
        }
        if (!/^Developer ID Application: .+ \([A-Z0-9]+\)$/.test(identity || '')) {
          throw new Error('Keychain notarization requires the full Developer ID Application identity.');
        }
        config.bundle ??= {};
        config.bundle.macOS ??= {};
        config.bundle.macOS.signingIdentity = identity;
        config.bundle.macOS.hardenedRuntime = true;
      } else if ((present(api) && present(account)) || !(complete(api) || complete(account))) {
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
  let check = false;
  const forwarded = [];
  for (let i = 0; i < args.length; i++) {
    const arg = args[i];
    if (arg === '--signed' || arg === '--unsigned') {
      if (mode) throw new Error('Choose one signing mode.');
      mode = arg;
      signed = arg === '--signed';
    } else if (arg === '--check' && !check) {
      check = true;
    } else if (['--target', '--bundles'].includes(arg)) {
      if (forwarded.includes(arg)) throw new Error(`Specify ${arg} only once.`);
      const value = args[++i];
      if (!value || value.startsWith('-')) throw new Error(`Missing value for ${arg}.`);
      forwarded.push(arg, value);
    } else {
      throw new Error('Usage: pnpm desktop:package [--unsigned|--signed] [--check] [--target TRIPLE] [--bundles FORMAT,...]');
    }
  }
  if (check && !signed) throw new Error('--check requires --signed.');
  return { signed, forwarded, ...(check ? { check } : {}) };
}

async function main() {
  const { signed, forwarded, check } = parseArguments(process.argv.slice(2));
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
  const target = forwarded.includes('--target') ? forwarded[forwarded.indexOf('--target') + 1] : undefined;
  const bundles = forwarded.includes('--bundles') ? forwarded[forwarded.indexOf('--bundles') + 1] : undefined;
  const keychain = signed && process.platform === 'darwin' && env.APPLE_NOTARY_PROFILE;
  if (keychain) await checkMacKeychain({ root, env, config, target, bundles });
  if (signed && process.platform === 'win32' && !config.bundle.windows.signCommand) {
    await runBuild('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', resolve(root, 'scripts/check-windows-signing.ps1'),
      '-CertificateThumbprint', config.bundle.windows.certificateThumbprint], { cwd: root, env });
  }
  if (check) {
    if (!keychain && process.platform !== 'win32') throw new Error('--check on macOS currently requires APPLE_NOTARY_PROFILE.');
    if (process.platform === 'win32' && config.bundle.windows.signCommand) throw new Error('Check custom signing providers using their own diagnostic command.');
    console.log('Signing prerequisites verified. No package was built or submitted.');
    return;
  }
  console.log(signed ? 'Building with local signing settings.' : 'Building without distribution signing or notarization.');
  // Keep merged signing settings (including provider commands) out of argv.
  let stage;
  try {
    let configArgument = JSON.stringify(config);
    if (signed) {
      const privateRoot = resolve(root, 'release/private');
      mkdirSync(privateRoot, { recursive: true });
      stage = mkdtempSync(resolve(privateRoot, 'signing-'));
      configArgument = resolve(stage, 'tauri.signing.json');
      writeFileSync(configArgument, JSON.stringify(config), { mode: 0o600 });
    }
    if (keychain) {
      await buildMacKeychainPackage({ root, env, config, cli, configArgument,
        target, bundles });
    } else {
      await runBuild(process.execPath, [cli, 'build', '--config', configArgument, ...forwarded, '--', '--locked'], { cwd: desktop, env });
    }
  } finally {
    if (stage) rmSync(stage, { recursive: true, force: true });
  }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch(error => { console.error(error.message); process.exitCode = 1; });
}
