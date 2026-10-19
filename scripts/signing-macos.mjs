import { spawn, execFile } from 'node:child_process';
import { readFileSync, mkdirSync, mkdtempSync, rmSync, copyFileSync, writeFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { resolve, join } from 'node:path';

export function runBuild(command, args, options) {
  return new Promise((accept, reject) => {
    const child = spawn(command, args, { ...options, stdio: 'inherit', shell: false });
    child.on('error', () => reject(new Error(`Could not launch ${command}.`)));
    child.on('exit', (code, signal) => code === 0 && !signal ? accept() : reject(new Error(`${command} failed; packaging stopped.`)));
  });
}

// Captured tools include notarization history. Never echo their output on errors.
function captureTool(command, args, options) {
  return new Promise((accept, reject) => execFile(command, args,
    { encoding: 'utf8', timeout: 45000, maxBuffer: 4 * 1024 * 1024, ...options },
    (error, stdout) => error ? reject(new Error(`${command} failed. Check signing identity, Keychain access and network connectivity.`)) : accept(stdout)));
}

export function macPackageOptions({ target, bundles, arch = process.arch }) {
  target ??= arch === 'arm64' ? 'aarch64-apple-darwin' : 'x86_64-apple-darwin';
  const architectures = {
    'aarch64-apple-darwin': ['arm64'],
    'x86_64-apple-darwin': ['x86_64'],
    'universal-apple-darwin': ['arm64', 'x86_64'],
  };
  if (!architectures[target]) throw new Error('Keychain signing requires a macOS Rust target.');
  const formats = (bundles ?? 'dmg').split(',');
  if (formats.some(format => !['app', 'dmg'].includes(format))) throw new Error('Keychain signing supports only app and dmg bundles.');
  return { target, architectures: architectures[target], dmg: formats.includes('dmg') };
}

export async function checkMacKeychain({ root, env, config, target, bundles, capture = captureTool }) {
  const options = macPackageOptions({ target, bundles });
  const identity = config.bundle.macOS.signingIdentity;
  const identities = await capture('security', ['find-identity', '-v', '-p', 'codesigning'], { cwd: root, env });
  if (!identities.includes(`"${identity}"`)) throw new Error('Import the configured Developer ID certificate AND private key into Keychain first.');
  for (const tool of ['notarytool', 'stapler', 'clang', 'lipo']) await capture('xcrun', ['--find', tool], { cwd: root, env });
  await capture('xcrun', ['notarytool', 'history', '--keychain-profile', env.APPLE_NOTARY_PROFILE, '--output-format', 'json'], { cwd: root, env });
  const installed = (await capture('rustup', ['target', 'list', '--installed'], { cwd: root, env })).split(/\s+/);
  const required = options.target === 'universal-apple-darwin' ? ['aarch64-apple-darwin', 'x86_64-apple-darwin'] : [options.target];
  for (const rustTarget of required) if (!installed.includes(rustTarget)) throw new Error(`Install the Rust target first: rustup target add ${rustTarget}`);
}

export async function buildMacKeychainPackage({ root, env, config, cli, configArgument, target, bundles,
  run = runBuild, capture = captureTool }) {
  const options = macPackageOptions({ target, bundles });
  const desktop = resolve(root, 'apps/desktop');
  const appConfig = JSON.parse(readFileSync(join(desktop, 'src-tauri/tauri.conf.json'), 'utf8'));
  const identity = config.bundle.macOS.signingIdentity;
  const team = identity.match(/\(([A-Z0-9]+)\)$/)[1];
  const buildEnv = { ...env };
  // This module owns notarization; Tauri only builds and signs the application.
  for (const key of ['APPLE_NOTARY_PROFILE', 'APPLE_ID', 'APPLE_PASSWORD', 'APPLE_TEAM_ID',
    'APPLE_API_KEY', 'APPLE_API_ISSUER', 'APPLE_API_KEY_PATH']) delete buildEnv[key];
  await run(process.execPath, [cli, 'build', '--config', configArgument, '--target', options.target, '--bundles', 'app', '--', '--locked'], { cwd: desktop, env: buildEnv });
  const metadata = JSON.parse(await capture('cargo', ['metadata', '--format-version', '1', '--no-deps', '--locked', '--manifest-path', 'apps/desktop/src-tauri/Cargo.toml'], { cwd: root, env: buildEnv }));
  const app = join(metadata.target_directory, options.target, 'release/bundle/macos', `${appConfig.productName}.app`);
  const executable = (await capture('/usr/libexec/PlistBuddy', ['-c', 'Print :CFBundleExecutable', join(app, 'Contents/Info.plist')], { cwd: root, env })).trim();
  const actual = (await capture('lipo', ['-archs', join(app, 'Contents/MacOS', executable)], { cwd: root, env })).trim().split(/\s+/).sort();
  if (actual.join(',') !== [...options.architectures].sort().join(',')) throw new Error('The built application has unexpected CPU architectures.');
  const tool = (command, args) => run(command, args, { cwd: root, env });
  await tool('codesign', ['--verify', '--deep', '--strict', app]);
  await tool('codesign', ['--verify', '-R', `=identifier "${appConfig.identifier}" and anchor apple generic and certificate leaf[subject.OU] = "${team}"`, app]);
  const privateRoot = resolve(root, 'release/private');
  mkdirSync(privateRoot, { recursive: true });
  const stage = mkdtempSync(join(privateRoot, 'notarize-'));
  async function notarize(file) {
    // notarytool bounds the wait; execFile remains asynchronous while Apple works.
    const result = JSON.parse(await capture('xcrun', ['notarytool', 'submit', file,
      '--keychain-profile', env.APPLE_NOTARY_PROFILE, '--wait', '--timeout', '30m', '--output-format', 'json'], { cwd: root, env, timeout: 31 * 60000 }));
    if (result.status !== 'Accepted') throw new Error(`Apple notarization was not accepted (${result.status}); submission ${result.id}. No release artifact was exported.`);
  }
  try {
    const zip = join(stage, 'application.zip');
    await tool('ditto', ['-c', '-k', '--keepParent', app, zip]);
    await notarize(zip);
    await tool('xcrun', ['stapler', 'staple', app]);
    await tool('xcrun', ['stapler', 'validate', app]);
    await tool('spctl', ['--assess', '--type', 'execute', app]);
    const output = resolve(root, 'release/installers', appConfig.version, options.target);
    if (!options.dmg) {
      mkdirSync(output, { recursive: true });
      await tool('ditto', [app, join(output, `${appConfig.productName}.app`)]);
      console.log(`Verified notarized application: ${output}`);
      return;
    }
    const image = join(stage, 'image');
    mkdirSync(image);
    await tool('ditto', [app, join(image, `${appConfig.productName}.app`)]);
    await tool('ln', ['-s', '/Applications', join(image, 'Applications')]);
    const name = `${appConfig.productName}_${appConfig.version}_${options.target.replace('-apple-darwin', '')}.dmg`;
    const dmg = join(stage, name);
    await tool('hdiutil', ['create', '-volname', appConfig.productName, '-srcfolder', image, '-format', 'UDZO', dmg]);
    await tool('codesign', ['--sign', identity, '--timestamp', dmg]);
    await tool('codesign', ['--verify', '--strict', dmg]);
    await notarize(dmg);
    await tool('xcrun', ['stapler', 'staple', dmg]);
    await tool('xcrun', ['stapler', 'validate', dmg]);
    await tool('spctl', ['--assess', '--type', 'open', '--context', 'context:primary-signature', dmg]);
    mkdirSync(output, { recursive: true });
    copyFileSync(dmg, join(output, name));
    writeFileSync(join(output, `${name}.sha256`), `${createHash('sha256').update(readFileSync(dmg)).digest('hex')}  ${name}\n`);
    console.log(`Verified signed and notarized installer: ${join(output, name)}`);
  } finally {
    rmSync(stage, { recursive: true, force: true });
  }
}
