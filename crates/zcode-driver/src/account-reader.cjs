// Native account projection. Secrets cross local child-process pipes only.
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const crypto = require('node:crypto');
const native = typeof __dogcodesNative !== 'undefined' ? __dogcodesNative : require('./credentials.cjs');
const captcha = typeof __dogcodesCaptcha !== 'undefined' ? __dogcodesCaptcha : require('./account-captcha.cjs');
const quotaWindows = typeof __dogcodesQuota !== 'undefined' ? __dogcodesQuota : require('./quota-reader.cjs');
const MAX_BYTES = 1024 * 1024;
const hash = value => crypto.createHash('sha256').update(value).digest('hex');
const fingerprint = value => hash(JSON.stringify(value));

function appVersion(builtinFile) {
  let fd;
  try {
    fd = fs.openSync(path.join(path.dirname(builtinFile), '..', '..', 'app.asar'), 'r');
    const fixed = Buffer.alloc(16); if (fs.readSync(fd, fixed, 0, 16, 0) !== 16) return 'unknown';
    const headerSize = fixed.readUInt32LE(4), size = fixed.readUInt32LE(12);
    if (size > 8 * MAX_BYTES || headerSize < size + 8) return 'unknown';
    const header = Buffer.alloc(size); if (fs.readSync(fd, header, 0, size, 16) !== size) return 'unknown';
    const entry = JSON.parse(header).files?.['package.json'];
    if (!entry || !Number.isSafeInteger(entry.size) || entry.size > 65_536 || !/^\d+$/.test(entry.offset)) return 'unknown';
    const body = Buffer.alloc(entry.size);
    if (fs.readSync(fd, body, 0, entry.size, 8 + headerSize + Number(entry.offset)) !== entry.size) return 'unknown';
    const version = JSON.parse(body).version;
    return typeof version === 'string' ? version : 'unknown';
  } catch { return 'unknown'; } finally { if (fd != null) fs.closeSync(fd); }
}

function readJson(file, missing = {}) {
  try {
    if (fs.statSync(file).size > MAX_BYTES) throw new Error('account_unavailable');
    return JSON.parse(fs.readFileSync(file, 'utf8'));
  } catch (error) {
    if (error.code === 'ENOENT') return missing;
    throw new Error('account_unavailable');
  }
}

function locations(env) {
  return {
    data: path.join(env.ZCODE_DATA_BASE_DIR || env.HOME || os.homedir(), '.zcode', 'v2'),
    settings: path.join(env.ZCODE_DESKTOP_HOME_DIR || env.HOME || os.homedir(), '.zcode', 'v2', 'setting.json'),
  };
}

function credentials(env) {
  const values = readJson(path.join(locations(env).data, 'credentials.json'));
  return key => typeof values[key] === 'string' && values[key].trim() ? native.credential(values[key]).trim() : '';
}

function context(env, providerId, mode) {
  const get = credentials(env);
  const family = get('oauth:active_provider');
  if (!['zai', 'bigmodel'].includes(family)) return undefined;
  if (mode === 'start-plan') {
    const apiKey = get('zcodejwttoken');
    const deviceMid = readJson(path.join(locations(env).data, 'telemetry-state.json')).deviceMid;
    if (!apiKey || typeof deviceMid !== 'string' || !deviceMid.trim()) return undefined;
    return { family, apiKey, deviceMid, current: true, fingerprint: fingerprint([family, apiKey, deviceMid]) };
  }
  if (mode !== 'individual-coding-plan') return undefined;
  const raw = get(`oauth:${family}:user_info`);
  if (!raw) return undefined;
  const profile = JSON.parse(raw);
  const identity = typeof profile.id === 'string' ? profile.id.trim()
    : family === 'zai' && typeof profile.user_id === 'string' ? profile.user_id.trim() : '';
  if (!identity || identity === 'unknown') return undefined;
  const apiKey = get(`account-provider:coding-plan:${providerId}:account:${encodeURIComponent(identity)}:api-key`);
  const settings = readJson(locations(env).settings);
  const selection = settings.providerFamilyConnectionSelections?.[family];
  const current = settings.providerFamilyDomain === family && (Object.hasOwn(settings, 'providerFamilyConnectionSelections')
    ? selection?.kind === mode
    : settings.modelProviderFamilyModes?.[family] !== 'apiKey'
      && settings.modelProviderFamilySelectedKeys?.[family] === `coding-plan:builtin:${family}-coding-plan`);
  const scope = fingerprint([family, identity, mode]);
  return { family, apiKey, current, scope, fingerprint: fingerprint([family, identity, apiKey, current]) };
}

function origin(value) {
  const url = new URL(value);
  if (url.username || url.password || (url.protocol !== 'https:'
    && !(url.protocol === 'http:' && ['127.0.0.1', 'localhost', '[::1]'].includes(url.hostname)))) throw new Error('account_unavailable');
  return url.origin;
}

function businessOrigin(family, env) {
  const prefix = family === 'zai' ? 'ZAI' : 'BIGMODEL';
  const suffix = family === 'zai' ? 'BUSINESS_BASE_URL' : 'API_BASE_URL';
  const test = env.ZCODE_ENV?.toLowerCase() === 'test';
  return origin(env[`${prefix}_${suffix}`] || env[`${prefix}_${test ? 'TEST' : 'PRODUCTION'}_${suffix}`]
    || (family === 'zai' ? (test ? 'https://api.chatglm.site' : 'https://api.z.ai') : (test ? 'https://dev.bigmodel.cn' : 'https://bigmodel.cn')));
}

async function query(url, headers, fetcher) {
  try {
    const response = await fetcher(url, { headers, method: 'GET', redirect: 'error', signal: AbortSignal.timeout(8000) });
    if ([401, 403].includes(response.status)) return { status: 'credential-failed' };
    if (!response.ok) return { status: 'unknown' };
    const reader = response.body.getReader();
    const chunks = []; let length = 0;
    try {
      while (true) {
        const { done, value } = await reader.read(); if (done) break;
        length += value.length; if (length > MAX_BYTES) return { status: 'unknown' };
        chunks.push(Buffer.from(value));
      }
    } finally { await reader.cancel().catch(() => {}); }
    const body = JSON.parse(Buffer.concat(chunks).toString('utf8'));
    if (body.success === false || (body.code != null && ![0, 200].includes(body.code))) return { status: 'unknown' };
    return { status: 'ok', data: body.data };
  } catch { return { status: 'unknown' }; }
}

function startModels(data, ids) {
  if (!Array.isArray(data?.plans) || !Array.isArray(data.balances)) return [];
  const now = Number.isFinite(data.server_time) ? data.server_time : Date.now() / 1000;
  const plans = data.plans.filter(p => {
    const names = [p.plan_id, p.name].filter(v => typeof v === 'string' && v.trim());
    return p.status?.toLowerCase() === 'active' && (!names.length || names.some(v => /start[- ]plan/i.test(v)))
      && !(Number(p.ends_at) > 0 && Number(p.ends_at) <= now);
  });
  const result = new Set();
  for (const b of data.balances) {
    if (!plans.some(p => b.user_plan_id && p.user_plan_id ? b.user_plan_id === p.user_plan_id
      : b.plan_id ? b.plan_id === p.plan_id : !b.user_plan_id)) continue;
    const caps = Array.isArray(b.capabilities) ? b.capabilities.filter(v => typeof v === 'string' && /^model:/i.test(v)).map(v => v.slice(6).trim()) : [];
    for (const name of caps.length ? caps : [b.show_name]) {
      const id = ids.find(v => typeof name === 'string' && v.toLowerCase() === name.trim().toLowerCase());
      if (id) result.add(id);
    }
  }
  return [...result];
}

async function inspect(input, env = process.env, fetcher = fetch) {
  const version = input.version || appVersion(input.builtinFile);
  const builtin = readJson(input.builtinFile);
  const rules = builtin.config?.providerConfigRules?.providerRules;
  if (builtin.revision == null || !Array.isArray(rules)) throw new Error('account_unavailable');
  const providers = {}, states = {}, authorized = {};
  // Queries for a family are shared across its provider rules during this read.
  const queried = new Map();
  for (const rule of rules) {
    const { mode, accountType } = rule.config?.access || {};
    if (!['start-plan', 'individual-coding-plan'].includes(mode)) continue;
    const id = rule.providerId;
    if (typeof id !== 'string') continue;
    let before;
    let status = 'unknown', allowed = [];
    try {
      before = context(env, id, mode);
      if (!before || before.family !== accountType) status = 'not-connected';
      else if (!before.apiKey) status = 'credential-failed';
      else {
        const key = fingerprint([mode, before.family, before.apiKey]);
        if (!queried.has(key)) {
          const url = mode === 'start-plan'
            ? `${origin(env.ZCODE_BASE_URL || 'https://zcode.z.ai')}/api/v1/zcode-plan/billing/balance?app_version=${encodeURIComponent(version)}`
            : `${businessOrigin(before.family, env)}/api/biz/subscription/list`;
          queried.set(key, query(url, mode === 'start-plan'
            ? { Authorization: `Bearer ${before.apiKey}`, 'X-Device-Mid': before.deviceMid }
            : { Authorization: before.apiKey }, fetcher));
        }
        const result = await queried.get(key);
        status = result.status;
        if (status === 'ok') {
          if (mode === 'start-plan') {
            allowed = startModels(result.data, rule.config.builtinModelIds || []);
            status = allowed.length ? 'available' : 'not-entitled';
          } else if (!Array.isArray(result.data)) status = 'unknown';
          else status = result.data.some(p => [p.productId, p.productName].some(v => typeof v === 'string' && /coding/i.test(v))
            && p.status === 'VALID' && p.inCurrentPeriod === true) ? 'available' : 'not-entitled';
        }
        const after = context(env, id, mode);
        if (!after || after.fingerprint !== before.fingerprint) { before = undefined; status = 'unknown'; }
      }
    } catch { status = 'unknown'; }
    const entitled = status === 'available';
    providers[id] = { access: { type: 'zhipu-account', entitled }, ...(allowed.length ? { builtinModelIds: allowed } : {}) };
    states[id] = { availability: entitled ? 'available' : status === 'unknown' ? 'unknown' : 'unavailable', entitled,
      current: before?.family === accountType && before.current === true,
      ...(before?.scope ? { connectionKey: before.scope } : {}),
      ...(!entitled && status !== 'unknown' ? { unavailableReason: status } : {}) };
    if (entitled && states[id].current) authorized[id] = {
      family: before.family, mode, fingerprint: before.fingerprint, scope: before.scope,
      ...(mode === 'start-plan' ? { models: allowed } : {}),
    };
  }
  return { overlay: {
    revision: `dogcodes:${builtin.revision}:${fingerprint([providers, states])}`,
    basedOnZCodeBuiltinRevision: `zcode-builtin:${builtin.revision}:${hash(path.resolve(input.builtinFile))}`,
    providers, states,
  }, authorized, checkedAt: Date.now(), version };
}

async function runtime(input, env = process.env, verifier = captcha.verify) {
  const reject = message => ({ headersApplied: false, errorMessage: message });
  const request = input.request || {}, id = request.providerId, grant = input.authorized?.[id];
  const access = request.accountAccess;
  if (!grant || access?.type !== 'zhipu-account' || access.accountType !== grant.family || access.mode !== grant.mode
    || request.modelSelection?.providerId !== id || (grant.models && !grant.models.includes(request.modelSelection?.modelId))) {
    return reject('请在 ZCode 中登录并选择已授权的个人套餐，然后重新检测 Agent。团队套餐尚未接入。');
  }
  try {
    // An old grant cannot silently cross logout, account, selection or key changes.
    const current = context(env, id, grant.mode);
    if (!current?.current || current.fingerprint !== grant.fingerprint
      || Date.now() - input.checkedAt > 5 * 60_000
      || (request.expectedAccountScope && request.expectedAccountScope !== current.scope)) {
      return reject('ZCode 账号或套餐已改变，请重新检测 Agent 后重试。');
    }
    const headers = grant.mode === 'start-plan' ? await verifier(input.version) : undefined;
    const after = context(env, id, grant.mode);
    if (!after || after.fingerprint !== current.fingerprint) return reject('ZCode 账号已改变，请重新检测 Agent。');
    return { headersApplied: true, requestAuth: { apiKey: after.apiKey,
      ...(after.scope ? { accountScope: after.scope } : {}), ...(headers ? { headers } : {}) } };
  } catch { return reject('ZCode 账号验证未完成，请检查原生登录状态或重试。'); }
}

function quotaIdentity(input, env = process.env) {
  try {
    if (!credentials(env)('oauth:active_provider')) return { unavailable: 'quota_login_required' };
    const builtin = readJson(input.builtinFile);
    const rules = builtin.config?.providerConfigRules?.providerRules || [];
    for (const rule of rules) {
      const access = rule.config?.access;
      if (access?.mode !== 'individual-coding-plan') continue;
      const current = context(env, rule.providerId, access.mode);
      if (current?.family !== access.accountType || !current?.current) continue;
      if (!current.apiKey) return { unavailable: 'quota_login_required' };
      return { identity: current.fingerprint, providerId: rule.providerId };
    }
    return { unavailable: 'quota_unavailable' };
  } catch { return { unavailable: 'quota_login_required' }; }
}

async function readQuota(input, env = process.env, fetcher = fetch) {
  const selected = quotaIdentity(input, env);
  if (selected.unavailable) return selected;
  try {
    const current = context(env, selected.providerId, 'individual-coding-plan');
    if (!current?.current || current.fingerprint !== selected.identity) return { unavailable: 'quota_login_required' };
    const host = current.family === 'zai' ? 'api.z.ai' : 'open.bigmodel.cn';
    const response = await query(`https://${host}/api/monitor/usage/quota/limit`, { Authorization: current.apiKey }, fetcher);
    const after = context(env, selected.providerId, 'individual-coding-plan');
    if (!after?.current || after.fingerprint !== selected.identity) return { unavailable: 'quota_login_required' };
    if (response.status === 'credential-failed') return { unavailable: 'quota_login_required' };
    if (response.status !== 'ok') return { unavailable: 'quota_unavailable' };
    const now = Date.now();
    return { identity: selected.identity, snapshot: {
      windows: quotaWindows.windowsFrom(response.data), context: null,
      collectedAtUnixMs: now, staleAfterUnixMs: now + 60000,
      planType: current.family === 'zai' ? 'Z.ai' : 'BigModel', source: 'native_credentials_pull',
      confidence: 'provider_reported', parserVersion: 'zcode-native-account-monitor-v2',
    } };
  } catch { return { unavailable: 'quota_unavailable' }; }
}

async function accountMain() {
  try {
    const chunks = []; let length = 0;
    for await (const chunk of process.stdin) {
      length += chunk.length; if (length > MAX_BYTES) throw new Error('account_unavailable'); chunks.push(chunk);
    }
    const input = JSON.parse(Buffer.concat(chunks).toString('utf8'));
    const output = input.action === 'inspect' ? await inspect(input) : input.action === 'runtime' ? await runtime(input)
      : input.action === 'quota_identity' ? quotaIdentity(input) : input.action === 'quota' ? await readQuota(input) : undefined;
    if (!output) throw new Error('account_unavailable');
    process.stdout.write(JSON.stringify(output));
  } catch { process.stdout.write(JSON.stringify({ error: 'account_unavailable' })); process.exitCode = 1; }
}
module.exports = { inspect, runtime, startModels, context, businessOrigin, quotaIdentity, readQuota, accountMain };
