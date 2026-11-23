const { test } = require('node:test');
const assert = require('node:assert/strict');
const crypto = require('node:crypto');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { credential, useKeyringForTests } = require('./credentials.cjs');

const sha256 = value => crypto.createHash('sha256').update(value).digest();
// Mirrors ZCode 0.18's writer so the reader is checked against its format.
function encryptV2(plain, key, source) {
  const iv = crypto.randomBytes(12);
  const cipher = crypto.createCipheriv('aes-256-gcm', key, iv);
  const bytes = Buffer.concat([cipher.update(plain, 'utf8'), cipher.final()]);
  const body = [iv, cipher.getAuthTag(), bytes].map(part => part.toString('base64url')).join('.');
  return `enc:v2:${source}:${sha256(key).toString('hex').slice(0, 24)}:${body}`;
}

function store(t) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'dogcodes-credentials-'));
  t.after(() => { fs.rmSync(root, { recursive: true, force: true }); useKeyringForTests(() => { throw new Error('unset'); }); });
  return path.join(root, 'credentials.json');
}

test('os-sourced v2 credentials use the keyring entry bound to the credentials file', t => {
  const file = store(t);
  const key = crypto.randomBytes(32);
  const lookups = [];
  useKeyringForTests(() => ({ Entry: class {
    constructor(service, account) { lookups.push([service, account]); }
    getPassword() { return key.toString('base64url'); }
  } }));
  const value = encryptV2('fixture-token', key, 'os');
  assert.equal(credential(value, { file, env: {} }), 'fixture-token');
  assert.equal(credential(value, { file, env: {} }), 'fixture-token');
  assert.deepEqual(lookups, [['ZCode Credential Encryption', sha256(path.resolve(file)).toString('hex')]]);
});

test('env-sourced v2 credentials accept the configured or managed secret', t => {
  const file = store(t);
  const secret = 's'.repeat(43);
  fs.writeFileSync(path.join(path.dirname(file), 'credential-secret'), secret, { mode: 0o600 });
  assert.equal(credential(encryptV2('managed', sha256(secret), 'env'), { file, env: {} }), 'managed');
  const other = store(t);
  assert.equal(credential(encryptV2('configured', sha256('configured-secret'), 'env'), {
    file: other, env: { ZCODE_CREDENTIAL_SECRET: 'configured-secret' },
  }), 'configured');
});

test('v2 credentials fail closed on a different key, tampering or a missing store', t => {
  const file = store(t);
  const key = crypto.randomBytes(32);
  useKeyringForTests(() => ({ Entry: class { getPassword() { return crypto.randomBytes(32).toString('base64url'); } } }));
  assert.throws(() => credential(encryptV2('x', key, 'os'), { file, env: {} }), /credential_key_mismatch/);

  const other = store(t);
  useKeyringForTests(() => ({ Entry: class { getPassword() { return key.toString('base64url'); } } }));
  const value = encryptV2('x', key, 'os');
  const tampered = value.slice(0, -2) + (value.endsWith('A') ? 'BB' : 'AA');
  assert.throws(() => credential(tampered, { file: other, env: {} }));
  assert.throws(() => credential(value, {}), /credential_format_unsupported/);
  assert.throws(() => credential('enc:v2:cloud:abc:def', { file: other, env: {} }), /credential_format_unsupported/);

  const missing = store(t);
  useKeyringForTests(() => ({ Entry: class { getPassword() { return null; } } }));
  assert.throws(() => credential(value, { file: missing, env: {} }), /credential_store_unavailable/);
});

test('v1 and plain credentials keep their previous behaviour', () => {
  const secret = 'legacy-secret';
  const key = sha256(secret);
  const iv = crypto.randomBytes(12);
  const cipher = crypto.createCipheriv('aes-256-gcm', key, iv);
  const bytes = Buffer.concat([cipher.update('legacy', 'utf8'), cipher.final()]);
  const value = `enc:v1:${[iv, cipher.getAuthTag(), bytes].map(part => part.toString('base64url')).join('.')}`;
  const previous = process.env.ZCODE_CREDENTIAL_SECRET;
  process.env.ZCODE_CREDENTIAL_SECRET = secret;
  try { assert.equal(credential(value), 'legacy'); } finally {
    if (previous === undefined) delete process.env.ZCODE_CREDENTIAL_SECRET; else process.env.ZCODE_CREDENTIAL_SECRET = previous;
  }
  assert.equal(credential('  plain  '), 'plain');
  assert.throws(() => credential(''), /login_required/);
});
