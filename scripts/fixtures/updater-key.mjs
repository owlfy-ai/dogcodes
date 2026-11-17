import { createHash, generateKeyPairSync, randomBytes, sign } from 'node:crypto';

// A throwaway in-memory key that signs like `tauri signer sign --app-version` (minisign,
// prehashed Ed25519). Tests only; nothing is written to disk.
export function testUpdaterKey() {
  const { publicKey, privateKey } = generateKeyPairSync('ed25519');
  const id = randomBytes(8);
  const raw = Buffer.from(publicKey.export({ format: 'jwk' }).x, 'base64url');
  const keyLine = Buffer.concat([Buffer.from('Ed'), id, raw]).toString('base64');
  const pubkey = Buffer.from(`untrusted comment: minisign public key: ${id.toString('hex').toUpperCase()}\n${keyLine}\n`).toString('base64');
  function signature(bytes, version, file = 'package') {
    const detached = sign(null, createHash('blake2b512').update(bytes).digest(), privateKey);
    const trusted = `timestamp:1700000000\tfile:${file}${version ? `\tversion:${version}` : ''}`;
    const global = sign(null, Buffer.concat([detached, Buffer.from(trusted)]), privateKey);
    const text = `untrusted comment: signature from tauri secret key\n${Buffer.concat([Buffer.from('ED'), id, detached]).toString('base64')}\ntrusted comment: ${trusted}\n${global.toString('base64')}\n`;
    return Buffer.from(text).toString('base64');
  }
  return { pubkey, signature };
}
