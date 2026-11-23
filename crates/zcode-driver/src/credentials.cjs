// Read-only decoding of ZCode Desktop credentials. Never log decoded values.
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const crypto = require('node:crypto');

// ZCode 0.18 writes `enc:v2:<source>:<keyId>:<iv>.<tag>.<ciphertext>`. An `os`
// key is 32 random bytes in the OS credential store, under this service and
// an account derived from the credentials file path; an `env` key is the
// SHA-256 of a managed secret. Both are only read here, never created.
const V2 = 'enc:v2:';
const KEYRING_SERVICE = 'ZCode Credential Encryption';
const KEY_BYTES = 32;
const sha256 = value => crypto.createHash('sha256').update(value).digest();
const keyId = key => sha256(key).toString('hex').slice(0, 24);

function decrypt(body, key) {
  const parts = body.split('.');
  if (parts.length !== 3 || parts.some(part => !/^[A-Za-z0-9_-]*$/.test(part))) throw new Error('credential_format_unsupported');
  const [iv, tag, bytes] = parts.map(part => Buffer.from(part, 'base64url'));
  if (iv.length !== 12 || tag.length !== 16) throw new Error('credential_format_unsupported');
  const cipher = crypto.createDecipheriv('aes-256-gcm', key, iv);
  cipher.setAuthTag(tag);
  return Buffer.concat([cipher.update(bytes), cipher.final()]).toString('utf8');
}

// The running ZCode binary ships its own keyring module. Loading it from that
// binary keeps the OS store's access rules: no new item, no extra prompt.
let loadKeyring = () => {
  const resources = process.resourcesPath;
  if (!resources) throw new Error('credential_store_unavailable');
  return require(path.join(resources, 'app.asar', 'node_modules', '@napi-rs', 'keyring'));
};

function osKey(file) {
  const { Entry } = loadKeyring();
  const secret = new Entry(KEYRING_SERVICE, sha256(path.resolve(file)).toString('hex')).getPassword();
  const key = typeof secret === 'string' ? Buffer.from(secret, 'base64url') : Buffer.alloc(0);
  if (key.length !== KEY_BYTES) throw new Error('credential_store_unavailable');
  return key;
}

function envKey(file, env) {
  if (env.ZCODE_CREDENTIAL_SECRET) return sha256(env.ZCODE_CREDENTIAL_SECRET);
  const secretFile = env.ZCODE_CREDENTIAL_SECRET_FILE || path.join(path.dirname(path.resolve(file)), 'credential-secret');
  const secret = fs.readFileSync(secretFile, 'utf8');
  const size = Buffer.byteLength(secret, 'utf8');
  if (size < 32 || size > 4096) throw new Error('credential_store_unavailable');
  return sha256(secret);
}

// One helper process reads one store; keep each source's key for its lifetime.
const keys = new Map();
function v2Key(source, file, env) {
  const cacheKey = `${source}\0${path.resolve(file)}`;
  if (!keys.has(cacheKey)) keys.set(cacheKey, source === 'os' ? osKey(file) : envKey(file, env));
  return keys.get(cacheKey);
}

function credential(value, store = {}) {
  if (typeof value !== 'string' || !value.trim()) throw new Error('login_required');
  if (!value.startsWith('enc:')) return value.trim();
  if (value.startsWith(V2)) {
    const [source, id, body, ...rest] = value.slice(V2.length).split(':');
    if (rest.length || !['os', 'env'].includes(source) || !/^[a-f0-9]{24}$/.test(id || '') || !body || !store.file) {
      throw new Error('credential_format_unsupported');
    }
    const key = v2Key(source, store.file, store.env || process.env);
    if (keyId(key) !== id) throw new Error('credential_key_mismatch');
    return decrypt(body, key);
  }
  if (!value.startsWith('enc:v1:')) throw new Error('credential_format_unsupported');
  const secret = process.env.ZCODE_CREDENTIAL_SECRET || `zcode-credential-fallback:${os.platform()}:${os.homedir()}:${os.userInfo().username}`;
  const key = crypto.createHash('sha256').update(secret).digest();
  try { return decrypt(value.slice(7), key); } finally { key.fill(0); }
}

// Tests substitute the OS store; production always uses the running binary's.
function useKeyringForTests(loader) { loadKeyring = loader; keys.clear(); }

module.exports = { credential, useKeyringForTests };
