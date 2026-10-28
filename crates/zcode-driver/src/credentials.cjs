// Read-only decoding of ZCode Desktop credentials. Never log decoded values.
const os = require('node:os');
const crypto = require('node:crypto');

function credential(value) {
  if (typeof value !== 'string' || !value.trim()) throw new Error('login_required');
  if (!value.startsWith('enc:')) return value.trim();
  if (!value.startsWith('enc:v1:')) throw new Error('credential_format_unsupported');
  const parts = value.slice(7).split('.');
  if (parts.length !== 3) throw new Error('credential_format_unsupported');
  const [iv, tag, bytes] = parts.map(part => Buffer.from(part, 'base64url'));
  if (iv.length !== 12 || tag.length !== 16) throw new Error('credential_format_unsupported');
  const secret = process.env.ZCODE_CREDENTIAL_SECRET || `zcode-credential-fallback:${os.platform()}:${os.homedir()}:${os.userInfo().username}`;
  const key = crypto.createHash('sha256').update(secret).digest();
  try {
    const cipher = crypto.createDecipheriv('aes-256-gcm', key, iv);
    cipher.setAuthTag(tag);
    return Buffer.concat([cipher.update(bytes), cipher.final()]).toString('utf8');
  } finally { key.fill(0); }
}

module.exports = { credential };
