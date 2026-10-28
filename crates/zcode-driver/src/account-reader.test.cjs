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

for (const family of ['bigmodel', 'zai']) test(`${family}: quota supports native OAuth without a persisted personal API key`, async t => {
  const f = fixture(t, family);
  delete f.store[f.keySlot];
  f.store[`oauth:${family}:access_token`] = 'fixture-login-token';
  // A different connection's key must never become a personal quota credential.
  f.store[`account-provider:coding-plan:account:${family}-team-coding-plan:account:fixture-user:api-key`] = 'wrong-team-key';
  f.write('credentials.json', f.store);
  const before = fs.readFileSync(path.join(f.root, '.zcode/v2/credentials.json'), 'utf8');
  const result = await readQuota(f.input, f.env, async (url, options) => {
    assert.equal(url, `${family === 'bigmodel' ? 'https://bigmodel.cn' : 'https://api.z.ai'}/api/monitor/usage/quota/limit`);
    assert.equal(options.method, 'GET');
    assert.equal(options.redirect, 'error');
    assert.equal(options.headers.Authorization, 'Bearer fixture-login-token');
    return Response.json({ code: 200, data: { limits: [
      { type: 'TOKENS_LIMIT', unit: 3, number: 5, percentage: 1 },
      { type: 'TOKENS_LIMIT', unit: 6, number: 1, percentage: 10 },
    ] } });
  });
  assert.deepEqual(result.snapshot.windows.map(w => w.usedPercent), [1, 10]);
  assert.equal(result.identity, quotaIdentity(f.input, f.env).identity);
  assert.ok(!JSON.stringify(result).includes('fixture-login-token'));
  assert.equal(fs.readFileSync(path.join(f.root, '.zcode/v2/credentials.json'), 'utf8'), before);
  // A login token alone does not authorize models without project-token resolution.
  const grant = await inspect(f.input, f.env, () => { throw new Error('unexpected entitlement query'); });
  assert.equal(grant.authorized[f.id], undefined);
});

function projectFixture(t, family = 'bigmodel', cached = true) {
  const f = fixture(t, family);
  delete f.store[f.keySlot];
  f.store[`oauth:${family}:access_token`] = 'fixture-login-token';
  const baseUrl = family === 'bigmodel' ? 'https://bigmodel.cn' : 'https://api.z.ai';
  const location = { organizationId: 'fixture-org', projectId: 'fixture-project', apiKeyId: 'fixture-key-id' };
  const locationSlot = `project-key-location:v1:${encodeURIComponent(JSON.stringify([baseUrl, family, 'fixture-user', null, 1, 'non-team']))}`;
  if (cached) f.store[locationSlot] = JSON.stringify(location);
  f.write('credentials.json', f.store);
  const calls = [];
  let issued = 0;
  const fetcher = async (url, options) => {
    calls.push({ url, method: options.method });
    assert.equal(options.redirect, 'error');
    if (url.endsWith('/getCustomerInfo')) {
      assert.equal(options.headers.Authorization, 'fixture-login-token');
      return Response.json({ code: 200, data: { organizations: [
        { organizationId: 'team-org', organizationName: '默认机构', projects: [{ projectId: 'team-project', projectType: 2 }] },
        { organizationId: 'fixture-org', organizationName: '个人机构', projects: [{ projectId: 'fixture-project', projectType: 1 }] },
      ] } });
    }
    const keys = `${baseUrl}/api/biz/v1/organization/fixture-org/projects/fixture-project/api_keys`;
    if (url === keys) {
      assert.equal(options.method, 'GET');
      assert.equal(options.headers.Authorization, family === 'zai' ? 'Bearer fixture-login-token' : 'fixture-login-token');
      return Response.json({ code: 0, data: [{ name: 'other-key', apiKey: 'wrong-key' },
        { name: 'zcode-api-key', apiKey: 'fixture-key-id', keyType: 1 }] });
    }
    if (url === `${keys}/fixture-key-id/access_tokens`) {
      assert.equal(options.method, 'POST');
      assert.equal(options.headers.Authorization, 'Bearer fixture-login-token');
      assert.deepEqual(JSON.parse(options.body), { clientType: 'zcode', clientVersion: '0.fixture' });
      return Response.json({ code: 200, data: { enabled: true, accessToken: `fixture-project-token-${++issued}`,
        tokenType: 'Bearer', expiresIn: 18000, expiresAt: Math.floor(Date.now() / 1000) + 18000 } });
    }
    if (url.endsWith('/subscription/list')) {
      assert.match(options.headers.Authorization, /^fixture-project-token-/);
      return Response.json({ code: 0, data: [{ productName: 'Coding Plan', status: 'VALID', inCurrentPeriod: true }] });
    }
    if (url.includes('/billing/balance')) return Response.json({ code: 0, data: { plans: [], balances: [] } });
    throw new Error('unexpected native request');
  };
  return { ...f, baseUrl, location, locationSlot, calls, fetcher };
}

for (const family of ['bigmodel', 'zai']) for (const cached of [false, true]) {
  test(`${family}: personal project authentication supports ${cached ? 'native location cache' : 'personal project discovery'}`, async t => {
    const f = projectFixture(t, family, cached);
    const before = fs.readFileSync(path.join(f.root, '.zcode/v2/credentials.json'), 'utf8');
    const grant = await inspect(f.input, f.env, f.fetcher);
    assert.equal(grant.overlay.states[f.id].availability, 'available');
    assert.equal(grant.overlay.states[f.id].current, true);
    assert.ok(grant.authorized[f.id]);
    for (const secret of ['fixture-login-token', 'fixture-project-token', 'fixture-user', 'fixture-key-id']) {
      assert.ok(!JSON.stringify(grant).includes(secret));
    }
    const scope = require('node:crypto').createHash('sha256').update(JSON.stringify([
      family, 'individual-coding-plan', null, 'fixture-login-token', 'fixture-start-jwt',
    ])).digest('hex');
    const request = { ...f.request, expectedAccountScope: scope };
    const auth = await runtime({ ...grant, request }, f.env, () => { throw new Error('must not verify'); }, f.fetcher);
    assert.equal(auth.headersApplied, true);
    assert.equal(auth.requestAuth.apiKey, 'fixture-project-token-2');
    assert.equal(auth.requestAuth.apiKeyId, 'fixture-key-id');
    assert.equal(auth.requestAuth.accountScope, scope);
    assert.equal(fs.readFileSync(path.join(f.root, '.zcode/v2/credentials.json'), 'utf8'), before);
    assert.ok(f.calls.every(call => call.method === 'GET' || call.url.endsWith('/access_tokens')));
    // Each callback resolves fresh material, including retries of a rejected token.
    const retry = await runtime({ ...grant, request: { ...request, rejectedProjectTokenFingerprint: 'rejected-token' } },
      f.env, undefined, f.fetcher);
    assert.equal(retry.requestAuth.apiKey, 'fixture-project-token-3');
  });
}

test('personal project grants cannot cross login, native scope, account or selected mode changes', async t => {
  const f = projectFixture(t);
  const grant = await inspect(f.input, f.env, f.fetcher);
  const changes = [
    () => f.write('credentials.json', { ...f.store, 'oauth:bigmodel:access_token': 'rotated-login' }),
    () => f.write('credentials.json', { ...f.store, zcodejwttoken: 'rotated-start-jwt' }),
    () => f.write('credentials.json', { ...f.store, 'oauth:bigmodel:user_info': JSON.stringify({ id: 'other-user' }) }),
    () => f.write('setting.json', { ...f.selection, providerFamilyConnectionSelections: { bigmodel: { kind: 'team-coding-plan' } } }),
  ];
  for (const change of changes) {
    f.write('credentials.json', f.store); f.write('setting.json', f.selection);
    change();
    const result = await runtime({ ...grant, request: f.request }, f.env, undefined,
      () => { throw new Error('changed scope must not issue a token'); });
    assert.equal(result.headersApplied, false);
  }
  f.write('credentials.json', f.store); f.write('setting.json', f.selection);
  const wrongScope = await runtime({ ...grant, request: { ...f.request, expectedAccountScope: 'wrong-scope' } }, f.env, undefined,
    () => { throw new Error('wrong scope must not issue a token'); });
  assert.equal(wrongScope.headersApplied, false);
});

test('account changes during project discovery or token issuance discard results before further requests', async t => {
  const f = projectFixture(t);
  for (const stage of ['/getCustomerInfo', '/access_tokens', '/subscription/list']) {
    f.write('credentials.json', f.store);
    let changed = false;
    const grant = await inspect(f.input, f.env, async (url, options) => {
      if (changed && !url.includes('/billing/balance')) throw new Error('continued using a changed account');
      const response = await f.fetcher(url, options);
      if (url.endsWith(stage)) {
        f.write('credentials.json', { ...f.store, 'oauth:bigmodel:access_token': 'rotated-login' });
        changed = true;
      }
      return response;
    });
    assert.equal(changed, true);
    assert.equal(grant.authorized[f.id], undefined);
    assert.equal(grant.overlay.states[f.id].entitled, false);
  }
});

test('personal project authentication rejects cached Team projects and never creates persistent Keys', async t => {
  const f = projectFixture(t);
  f.store[f.locationSlot] = JSON.stringify({ ...f.location, organizationId: 'team-org', projectId: 'team-project' });
  f.write('credentials.json', f.store);
  const team = await inspect(f.input, f.env, f.fetcher);
  assert.equal(team.authorized[f.id], undefined);
  assert.ok(!f.calls.some(call => call.method === 'POST'));
  delete f.store[f.locationSlot]; f.write('credentials.json', f.store); f.calls.length = 0;
  const missing = await inspect(f.input, f.env, async (url, options) => url.endsWith('/api_keys')
    ? Response.json({ code: 200, data: [] }) : f.fetcher(url, options));
  assert.equal(missing.authorized[f.id], undefined);
  assert.ok(!f.calls.some(call => call.method === 'POST'));
});

test('stale native location is rediscovered once after a deleted Key response', async t => {
  const f = projectFixture(t);
  const grant = await inspect(f.input, f.env, async (url, options) => {
    if (url.endsWith('/access_tokens') && !f.calls.some(call => call.url.endsWith('/api_keys'))) {
      return Response.json({ code: 404 });
    }
    return f.fetcher(url, options);
  });
  assert.ok(grant.authorized[f.id]);
  assert.equal(f.calls.filter(call => call.url.endsWith('/api_keys')).length, 1);
});

test('a personal project grant cannot silently switch to another Key within the same account', async t => {
  const f = projectFixture(t);
  const grant = await inspect(f.input, f.env, f.fetcher);
  const auth = await runtime({ ...grant, request: f.request }, f.env, undefined, async (url, options) => {
    if (url.endsWith('/fixture-key-id/access_tokens')) return new Response('', { status: 404 });
    if (url.endsWith('/api_keys')) return Response.json({ code: 200, data: [{ name: 'zcode-api-key', apiKey: 'replacement-key' }] });
    if (url.endsWith('/replacement-key/access_tokens')) return Response.json({ code: 200, data: {
      accessToken: 'replacement-token', tokenType: 'Bearer', expiresIn: 18000, expiresAt: Date.now() / 1000 + 18000,
    } });
    return f.fetcher(url, options);
  });
  assert.equal(auth.headersApplied, false);
  assert.ok(!JSON.stringify(auth).includes('replacement-token'));
  const retry = await runtime({ ...grant, request: { ...f.request, rejectedProjectTokenFingerprint: 'rejected' } },
    f.env, undefined, () => { throw new Error('unscoped retry must not issue a token'); });
  assert.equal(retry.headersApplied, false);
});

test('disabled, expired, malformed and unauthorized project tokens never authorize models', async t => {
  const f = projectFixture(t);
  for (const response of [
    () => new Response('', { status: 401 }),
    () => Response.json({ code: 200, data: { enabled: false } }),
    () => Response.json({ code: 200, data: { enable: false, accessToken: 'secret' } }),
    () => Response.json({ code: 200, data: { accessToken: 'secret', tokenType: 'Bearer', expiresIn: 10, expiresAt: 1 } }),
    () => Response.json({ code: 200, data: { accessToken: 'secret', tokenType: 'wrong', expiresIn: 10, expiresAt: Date.now() / 1000 + 10 } }),
    () => Response.json({ code: 200, data: null }),
  ]) {
    const grant = await inspect(f.input, f.env, (url, options) => url.endsWith('/access_tokens') ? response() : f.fetcher(url, options));
    assert.equal(grant.authorized[f.id], undefined);
    assert.ok(!JSON.stringify(grant).includes('secret'));
  }
});

test('OAuth quota rejects login, account and connection changes while a request is in flight', async t => {
  const f = fixture(t);
  delete f.store[f.keySlot];
  f.store['oauth:bigmodel:access_token'] = 'Bearer fixture-login-token';
  for (const change of [
    () => f.write('credentials.json', { ...f.store, 'oauth:bigmodel:access_token': 'rotated-token' }),
    () => f.write('credentials.json', { ...f.store, 'oauth:bigmodel:access_token': '' }),
    () => f.write('credentials.json', { ...f.store, 'oauth:active_provider': '' }),
    () => f.write('credentials.json', { ...f.store, 'oauth:bigmodel:user_info': JSON.stringify({ id: 'other-account' }) }),
    () => f.write('setting.json', { ...f.selection, providerFamilyConnectionSelections: { bigmodel: { kind: 'team-coding-plan' } } }),
  ]) {
    f.write('credentials.json', f.store);
    f.write('setting.json', f.selection);
    const result = await readQuota(f.input, f.env, async (url, options) => {
      assert.equal(options.headers.Authorization, 'Bearer fixture-login-token');
      change();
      return Response.json({ code: 200, data: { limits: [{ type: 'TOKENS_LIMIT', unit: 3, number: 5, percentage: 1 }] } });
    });
    assert.equal(result.unavailable, 'quota_login_required');
  }
});

test('OAuth quota respects business origins and rejects expired login tokens', async t => {
  const f = fixture(t);
  delete f.store[f.keySlot];
  f.store['oauth:bigmodel:access_token'] = 'fixture-login-token';
  f.write('credentials.json', f.store);
  const result = await readQuota(f.input, { ...f.env, BIGMODEL_API_BASE_URL: 'http://127.0.0.1:1234' }, async (url) => {
    assert.equal(url, 'http://127.0.0.1:1234/api/monitor/usage/quota/limit');
    return new Response('', { status: 401 });
  });
  assert.equal(result.unavailable, 'quota_login_required');
  for (const kind of ['api-key', 'start-plan', 'team-coding-plan']) {
    f.write('setting.json', { ...f.selection, providerFamilyConnectionSelections: { bigmodel: { kind } } });
    assert.equal(quotaIdentity(f.input, f.env).unavailable, 'quota_unavailable');
  }
});
