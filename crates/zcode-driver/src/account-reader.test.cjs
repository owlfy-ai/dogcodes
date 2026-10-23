const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { inspect, runtime, startModels, businessOrigin, readQuota, quotaIdentity } = require('./account-reader.cjs');

function fixture(t, family = 'bigmodel') {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'dogcodes-account-'));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const data = path.join(root, '.zcode', 'v2'); fs.mkdirSync(data, { recursive: true });
  const id = `account:${family}-individual-coding-plan`;
  const start = `account:${family}-start-plan`;
  const keySlot = `account-provider:coding-plan:${id}:account:fixture-user:api-key`;
  const store = { 'oauth:active_provider': family, [`oauth:${family}:user_info`]: JSON.stringify({ id: 'fixture-user' }),
    [keySlot]: 'fixture-api-key', zcodejwttoken: 'fixture-start-jwt' };
  const write = (name, value) => fs.writeFileSync(path.join(data, name), JSON.stringify(value), { mode: 0o600 });
  write('credentials.json', store); write('telemetry-state.json', { deviceMid: 'fixture-device' });
  const selection = { providerFamilyDomain: family, providerFamilyConnectionSelections: { [family]: { kind: 'individual-coding-plan' } } };
  write('setting.json', selection);
  const builtinFile = path.join(root, 'builtin.json');
  fs.writeFileSync(builtinFile, JSON.stringify({ revision: 7, config: { providerConfigRules: { providerRules: [
    { providerId: id, config: { builtinModelIds: ['GLM-5.3', 'GLM-5.3-Flash'], access: { mode: 'individual-coding-plan', accountType: family } } },
    { providerId: start, config: { builtinModelIds: ['GLM-5.3', 'GLM-5.3-Flash'], access: { mode: 'start-plan', accountType: family } } },
    { providerId: 'team', config: { access: { mode: 'team-coding-plan', accountType: family } } },
  ] } } }));
  const env = { HOME: root };
  const input = { builtinFile, version: '0.fixture' };
  const request = { providerId: id, modelSelection: { providerId: id, modelId: 'GLM-5.3' },
    accountAccess: { type: 'zhipu-account', mode: 'individual-coding-plan', accountType: family } };
  const fetcher = async (url, options) => {
    assert.equal(options.redirect, 'error');
    if (String(url).includes('/subscription/list')) {
      assert.equal(options.headers.Authorization, 'fixture-api-key');
      return Response.json({ code: 0, data: [{ productName: 'GLM Coding Plan', status: 'VALID', inCurrentPeriod: true }] });
    }
    assert.equal(options.headers.Authorization, 'Bearer fixture-start-jwt');
    assert.equal(options.headers['X-Device-Mid'], 'fixture-device');
    return Response.json({ code: 0, data: { plans: [{ plan_id: 'start-plan-test', status: 'active' }],
      balances: [{ capabilities: ['model:glm-5.3'] }] } });
  };
  return { root, env, id, start, store, keySlot, input, request, fetcher, write, selection };
}

for (const family of ['bigmodel', 'zai']) test(`${family}: discovers full native provider and supplies scoped credentials`, async t => {
  const f = fixture(t, family);
  const grant = await inspect(f.input, f.env, f.fetcher);
  assert.equal(grant.overlay.providers[f.id].access.entitled, true);
  assert.equal(grant.overlay.states[f.id].current, true);
  assert.equal(grant.overlay.providers.team, undefined);
  const overlay = JSON.stringify(grant.overlay);
  assert.ok(!overlay.includes('fixture-api-key') && !overlay.includes('fixture-user') && !overlay.includes('fixture-start-jwt'));
  const auth = await runtime({ ...grant, request: f.request }, f.env, () => { throw new Error('Coding Plan must not verify'); });
  assert.equal(auth.headersApplied, true);
  assert.equal(auth.requestAuth.apiKey, 'fixture-api-key');
  assert.match(auth.requestAuth.accountScope, /^[a-f0-9]{64}$/);
});

test('rejects revoked key, logout, selected mode changes and cross-provider requests', async t => {
  const f = fixture(t); const grant = await inspect(f.input, f.env, f.fetcher);
  for (const request of [
    { ...f.request, providerId: 'team' },
    { ...f.request, modelSelection: { providerId: 'other', modelId: 'GLM-5.3' } },
    { ...f.request, accountAccess: { ...f.request.accountAccess, mode: 'team-coding-plan' } },
    { ...f.request, expectedAccountScope: '0'.repeat(64) },
  ]) assert.equal((await runtime({ ...grant, request }, f.env)).headersApplied, false);
  const original = { ...f.store };
  for (const changed of [{ ...original, [f.keySlot]: 'replacement' }, { ...original, 'oauth:active_provider': '' }]) {
    f.write('credentials.json', changed);
    assert.equal((await runtime({ ...grant, request: f.request }, f.env)).headersApplied, false);
  }
  f.write('credentials.json', original);
  f.write('setting.json', { ...f.selection, providerFamilyConnectionSelections: { bigmodel: { kind: 'team-coding-plan' } } });
  assert.equal((await runtime({ ...grant, request: f.request }, f.env)).headersApplied, false);
});

test('account change during entitlement query never publishes a mixed grant', async t => {
  const f = fixture(t);
  const grant = await inspect(f.input, f.env, async (url, options) => {
    const response = await f.fetcher(url, options);
    f.write('credentials.json', { ...f.store, 'oauth:active_provider': 'zai' });
    return response;
  });
  assert.equal(grant.overlay.states[f.id].entitled, false);
  assert.equal(grant.authorized[f.id], undefined);
});

test('network failure, malformed payload, expired subscription and rejected key fail closed independently', async t => {
  const f = fixture(t);
  for (const fetcher of [async () => { throw new Error('private-provider-data'); }, async () => Response.json({ data: null }),
    async () => Response.json({ data: [{ productName: 'Coding Plan', status: 'EXPIRED', inCurrentPeriod: false }] }),
    async () => new Response('', { status: 401 })]) {
    const grant = await inspect(f.input, f.env, fetcher);
    assert.equal(grant.overlay.states[f.id].entitled, false);
    assert.equal(grant.authorized[f.id], undefined);
    assert.ok(!JSON.stringify(grant).includes('private-provider-data'));
  }
});

test('Start Plan only exposes granted models and verifies each scoped runtime request', async t => {
  const f = fixture(t); const grant = await inspect(f.input, f.env, f.fetcher);
  assert.deepEqual(grant.overlay.providers[f.start].builtinModelIds, ['GLM-5.3']);
  let verified = 0;
  const request = { providerId: f.start, modelSelection: { providerId: f.start, modelId: 'GLM-5.3' },
    accountAccess: { type: 'zhipu-account', mode: 'start-plan', accountType: 'bigmodel' } };
  const verifier = async version => { verified++; assert.equal(version, '0.fixture'); return { 'X-Aliyun-Captcha-Verify-Param': 'fixture-proof' }; };
  const auth = await runtime({ ...grant, request }, f.env, verifier);
  assert.equal(auth.requestAuth.apiKey, 'fixture-start-jwt');
  assert.equal(auth.requestAuth.headers['X-Aliyun-Captcha-Verify-Param'], 'fixture-proof');
  assert.equal((await runtime({ ...grant, request: { ...request, modelSelection: { ...request.modelSelection, modelId: 'GLM-5.3-Flash' } } }, f.env, verifier)).headersApplied, false);
  assert.equal(verified, 1);
});

test('Start Plan drops expired plans and unknown capability model names', () => {
  const data = { server_time: 100, plans: [{ plan_id: 'start-plan', status: 'active', ends_at: 90 }], balances: [{ capabilities: ['model:GLM-5.3'] }] };
  assert.deepEqual(startModels(data, ['GLM-5.3']), []);
  data.plans[0].ends_at = 200;
  data.balances[0].capabilities.push('model:invented');
  assert.deepEqual(startModels(data, ['GLM-5.3']), ['GLM-5.3']);
});

test('stale grants and identity changes during verification are rejected', async t => {
  const f = fixture(t); const grant = await inspect(f.input, f.env, f.fetcher);
  assert.equal((await runtime({ ...grant, checkedAt: Date.now() - 300_001, request: f.request }, f.env)).headersApplied, false);
  const request = { providerId: f.start, modelSelection: { providerId: f.start, modelId: 'GLM-5.3' },
    accountAccess: { type: 'zhipu-account', mode: 'start-plan', accountType: 'bigmodel' } };
  const auth = await runtime({ ...grant, request }, f.env, async () => {
    f.write('credentials.json', { ...f.store, zcodejwttoken: 'changed-jwt' }); return {};
  });
  assert.equal(auth.headersApplied, false);
});

test('data and desktop home overrides are independent; insecure endpoint rejected', async t => {
  const f = fixture(t);
  const env = { HOME: '/unused', ZCODE_DATA_BASE_DIR: f.root, ZCODE_DESKTOP_HOME_DIR: f.root };
  assert.equal((await inspect(f.input, env, f.fetcher)).overlay.states[f.id].entitled, true);
  assert.throws(() => businessOrigin('zai', { ZAI_BUSINESS_BASE_URL: 'http://untrusted.example' }));
  assert.throws(() => businessOrigin('zai', { ZAI_BUSINESS_BASE_URL: 'https://user:password@example.com' }));
});

test('credential decryption failure does not expose secrets or authorize the account', async t => {
  const f = fixture(t); f.write('credentials.json', { 'oauth:active_provider': 'enc:v2:private-secret' });
  const grant = await inspect(f.input, f.env, f.fetcher);
  assert.deepEqual(grant.authorized, {});
  assert.ok(!JSON.stringify(grant).includes('private-secret'));
});

test('quota uses the selected native personal account and ignores obsolete CLI configuration', async t => {
  const f = fixture(t);
  const cli = path.join(f.root, '.zcode', 'cli'); fs.mkdirSync(cli);
  const old = path.join(cli, 'config.json');
  const previous = JSON.stringify({ model: 'obsolete/GLM-5.3-Flash', provider: { obsolete: {
    options: { apiKey: 'wrong-obsolete-key', baseURL: 'https://open.bigmodel.cn/api/anthropic' }, models: { 'GLM-5.3-Flash': {} },
  } } });
  fs.writeFileSync(old, previous, { mode: 0o600 });
  const native = await inspect(f.input, f.env, f.fetcher);
  assert.equal(native.overlay.states[f.id].entitled, true);
  const result = await readQuota(f.input, f.env, async (url, options) => {
    assert.equal(url, 'https://open.bigmodel.cn/api/monitor/usage/quota/limit');
    assert.equal(options.headers.Authorization, 'fixture-api-key');
    return Response.json({ code: 200, data: { limits: [{ type: 'TOKENS_LIMIT', unit: 3, number: 5, percentage: 13 }] } });
  });
  assert.equal(result.snapshot.windows[0].usedPercent, 13);
  assert.equal(result.identity, quotaIdentity(f.input, f.env).identity);
  assert.equal(fs.readFileSync(old, 'utf8'), previous);
  f.write('setting.json', { ...f.selection, providerFamilyConnectionSelections: { bigmodel: { kind: 'api-key' } } });
  assert.equal(quotaIdentity(f.input, f.env).unavailable, 'quota_unavailable');
});

test('quota cannot cross an account change or keep an unauthorized key', async t => {
  const f = fixture(t);
  const changed = await readQuota(f.input, f.env, async () => {
    f.write('credentials.json', { ...f.store, [f.keySlot]: 'replaced-key' });
    return Response.json({ code: 200, data: { limits: [{ type: 'TOKENS_LIMIT', unit: 3, number: 5, percentage: 13 }] } });
  });
  assert.equal(changed.unavailable, 'quota_login_required');
  const rejected = await readQuota(f.input, f.env, async () => new Response('', { status: 401 }));
  assert.equal(rejected.unavailable, 'quota_login_required');
  f.write('credentials.json', {});
  assert.equal(quotaIdentity(f.input, f.env).unavailable, 'quota_login_required');
});
