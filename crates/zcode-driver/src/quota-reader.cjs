// Read-only native configuration reuse, authorized by the user. Never log keys,
// provider payloads, mutate credentials, refresh tokens, or follow redirects.
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const crypto = require('node:crypto');
const LIMIT = 1024 * 1024;

function windowsFrom(data) {
  if (!Array.isArray(data?.limits)) throw new Error('invalid_payload');
  const windows = [];
  for (const limit of data.limits) {
    let minutes;
    if (['TOKENS_LIMIT', 'CREDIT_LIMIT'].includes(limit.type) && limit.unit === 3 && limit.number === 5) minutes = 300;
    else if (['TOKENS_LIMIT', 'CREDIT_LIMIT'].includes(limit.type) && limit.unit === 6 && limit.number === 1) minutes = 10080;
    else continue;
    const percent = limit.percentage;
    if (percent != null && (!Number.isFinite(percent) || percent < 0 || percent > 100)) throw new Error('invalid_payload');
    const reset = limit.nextResetTime;
    if (reset != null && (!Number.isSafeInteger(reset) || reset < 0)) throw new Error('invalid_payload');
    windows.push({ id: `${limit.type}:${limit.unit}:${limit.number}`, scope: null, windowMinutes: minutes,
      usedPercent: percent ?? null, resetsAtUnix: reset == null ? null : Math.floor(reset / 1000) });
  }
  if (!windows.length) throw new Error('invalid_payload');
  return windows.sort((a, b) => a.windowMinutes - b.windowMinutes);
}

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

function select(config) {
  if (typeof config.model !== 'string') throw new Error('model_selection_unknown');
  const id = config.model.split('/')[0];
  // Never borrow another enabled provider's key or select the first provider.
  const provider = config.provider?.[id];
  if (!provider || provider.enabled === false) throw new Error('model_selection_unknown');
  const url = new URL(provider.options?.baseURL);
  if (url.protocol !== 'https:' || url.port || url.username || url.password ||
      !['open.bigmodel.cn', 'api.z.ai'].includes(url.hostname)) throw new Error('provider_unsupported');
  return { url: `https://${url.hostname}/api/monitor/usage/quota/limit`, key: credential(provider.options.apiKey),
    family: url.hostname === 'api.z.ai' ? 'Z.ai' : 'BigModel' };
}

async function main() {
  try {
    const file = path.join(os.homedir(), '.zcode', 'cli', 'config.json');
    if (fs.statSync(file).size > LIMIT) throw new Error('config_unavailable');
    const selected = select(JSON.parse(fs.readFileSync(file, 'utf8')));
    const response = await fetch(selected.url, { method: 'GET', redirect: 'error',
      headers: { authorization: selected.key }, signal: AbortSignal.timeout(15000) });
    if ([401,403].includes(response.status)) throw new Error('login_required');
    if (!response.ok) throw new Error('quota_unavailable');
    const reader = response.body.getReader();
    const chunks = []; let size = 0;
    try {
      while (true) {
        const {done,value} = await reader.read(); if (done) break;
        size += value.length; if (size > LIMIT) throw new Error('invalid_payload');
        chunks.push(Buffer.from(value));
      }
    } finally { await reader.cancel().catch(() => {}); }
    const data = JSON.parse(Buffer.concat(chunks).toString('utf8'));
    if (data.code !== 200 || data.success === false) throw new Error('quota_unavailable');
    const windows = windowsFrom(data.data);
    const now = Date.now();
    console.log(JSON.stringify({ windows, context: null, collectedAtUnixMs: now, staleAfterUnixMs: now + 60000,
      planType: selected.family, source: 'native_credentials_pull', confidence: 'provider_reported', parserVersion: 'zcode-monitor-v1' }));
  } catch (error) {
    const allowed = ['invalid_payload','login_required','credential_format_unsupported','model_selection_unknown','provider_unsupported','config_unavailable','quota_unavailable'];
    console.log(JSON.stringify({ error: allowed.includes(error.message) ? error.message : 'quota_unavailable' }));
    process.exitCode = 1;
  }
}
module.exports = { windowsFrom, select, credential };
