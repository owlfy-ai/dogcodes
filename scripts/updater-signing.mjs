import { createHash, createPublicKey, verify } from 'node:crypto';
import { existsSync, readFileSync, statSync } from 'node:fs';
import { isAbsolute, resolve } from 'node:path';

// In-app update signatures (minisign, made by `tauri signer sign --app-version`). The private
// key only ever reaches the signer process through these variables, never argv or a build.
export const updaterKeyVariables = ['TAURI_SIGNING_PRIVATE_KEY', 'TAURI_SIGNING_PRIVATE_KEY_PATH', 'TAURI_SIGNING_PRIVATE_KEY_PASSWORD'];

export function withoutUpdaterKey(env) {
  const copy = { ...env };
  for (const key of updaterKeyVariables) delete copy[key];
  return copy;
}

/** The environment for the signer: exactly one key source plus its password. */
export function updaterSigningEnvironment(env, root) {
  const key = env.TAURI_SIGNING_PRIVATE_KEY?.trim();
  let path = env.TAURI_SIGNING_PRIVATE_KEY_PATH?.trim();
  if (Boolean(key) === Boolean(path)) {
    throw new Error('In-app update signing needs exactly one of TAURI_SIGNING_PRIVATE_KEY_PATH or TAURI_SIGNING_PRIVATE_KEY (see release/.env.example).');
  }
  if (path) {
    path = isAbsolute(path) ? path : resolve(root, path);
    if (!existsSync(path) || !statSync(path).isFile()) throw new Error('TAURI_SIGNING_PRIVATE_KEY_PATH does not name a readable key file.');
  }
  if (!env.TAURI_SIGNING_PRIVATE_KEY_PASSWORD) {
    throw new Error('Set TAURI_SIGNING_PRIVATE_KEY_PASSWORD for the in-app update key; the signer must not prompt.');
  }
  return { ...withoutUpdaterKey(env), ...(path ? { TAURI_SIGNING_PRIVATE_KEY_PATH: path } : { TAURI_SIGNING_PRIVATE_KEY: key }),
    TAURI_SIGNING_PRIVATE_KEY_PASSWORD: env.TAURI_SIGNING_PRIVATE_KEY_PASSWORD };
}

/** `plugins.updater.pubkey` from the committed app configuration. */
export function configuredUpdaterPubkey(root) {
  const config = JSON.parse(readFileSync(resolve(root, 'apps/desktop/src-tauri/tauri.conf.json'), 'utf8'));
  const pubkey = config.plugins?.updater?.pubkey?.trim();
  if (!pubkey) throw new Error('Set plugins.updater.pubkey in apps/desktop/src-tauri/tauri.conf.json before signing updates.');
  return pubkey;
}

export function workspaceVersion(root) {
  const version = readFileSync(resolve(root, 'Cargo.toml'), 'utf8').match(/\[workspace\.package\][^\[]*?\nversion\s*=\s*"([^"]+)"/)?.[1];
  if (!version) throw new Error('Cargo.toml [workspace.package] version is missing.');
  return version;
}

const lines = base64 => Buffer.from(base64.trim(), 'base64').toString('utf8').split('\n');
const ed25519 = raw => createPublicKey({ key: { kty: 'OKP', crv: 'Ed25519', x: raw.toString('base64url') }, format: 'jwk' });

/**
 * Checks a Tauri updater signature the way the app does (same key, valid global signature)
 * and returns the version it is bound to, or null when it names none.
 */
export function verifyUpdaterSignature({ bytes, signature, pubkey }) {
  const key = Buffer.from(lines(pubkey)[1] ?? '', 'base64');
  const [, encoded, comment, global] = lines(signature);
  const signed = Buffer.from(encoded ?? '', 'base64');
  const trusted = comment?.startsWith('trusted comment: ') ? comment.slice('trusted comment: '.length) : null;
  if (key.length !== 42 || key.subarray(0, 2).toString() !== 'Ed' || signed.length !== 74 || trusted === null) {
    throw new Error('Malformed updater key or signature.');
  }
  if (!signed.subarray(2, 10).equals(key.subarray(2, 10))) throw new Error('The update was signed with a different key than plugins.updater.pubkey.');
  const algorithm = signed.subarray(0, 2).toString();
  const message = algorithm === 'ED' ? createHash('blake2b512').update(bytes).digest() : algorithm === 'Ed' ? bytes : null;
  const publicKey = ed25519(key.subarray(10));
  const detached = signed.subarray(10);
  if (!message || !verify(null, message, publicKey, detached)) throw new Error('The update signature does not match the package.');
  if (!verify(null, Buffer.concat([detached, Buffer.from(trusted)]), publicKey, Buffer.from(global ?? '', 'base64'))) {
    throw new Error('The update signature comment is not signed.');
  }
  return trusted.split('\t').find(field => field.startsWith('version:'))?.slice('version:'.length) ?? null;
}

/** Signs `file` into `file.sig`, then checks it against the committed public key and version. */
export async function signUpdaterPackage({ root, cli, file, version, env, run }) {
  const signerEnv = updaterSigningEnvironment(env, root);
  const pubkey = configuredUpdaterPubkey(root);
  await run(process.execPath, [cli, 'signer', 'sign', '--app-version', version, file], { cwd: root, env: signerEnv });
  const signature = `${file}.sig`;
  if (!existsSync(signature)) throw new Error('The Tauri signer wrote no signature. Use @tauri-apps/cli 2.12.4 or newer.');
  const signedFor = verifyUpdaterSignature({ bytes: readFileSync(file), signature: readFileSync(signature, 'utf8'), pubkey });
  if (signedFor !== version) throw new Error(`The update signature is bound to ${signedFor ?? 'no version'}, not ${version}. Use @tauri-apps/cli 2.12.4 or newer.`);
  return signature;
}

/** A macOS updater archive must hold exactly the app bundle at its root, without AppleDouble files. */
export function checkAppArchiveEntries(listing, appName) {
  const entries = listing.split('\n').map(line => line.trim().replace(/^\.\//, '')).filter(Boolean);
  const stray = entries.filter(entry => !(entry === appName || entry === `${appName}/` || entry.startsWith(`${appName}/`))
    || entry.split('/').some(part => part.startsWith('._')));
  if (!entries.length || stray.length) throw new Error(`The updater archive must contain only ${appName}.`);
}
