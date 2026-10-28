const { test } = require('node:test');
const assert = require('node:assert/strict');
const { verify, challenge, page } = require('./account-captcha.cjs');

test('disabled or skipped verification never opens the browser', async () => {
  for (const captcha of [{ enabled: false }, { skip_model_request: true }, undefined]) {
    assert.deepEqual(await verify('fixture', async () => Response.json({ data: { configs: { captcha } } }),
      () => { throw new Error('must not open browser'); }), {});
  }
});
test('configuration network failure does not pretend verification succeeded', async () => {
  await assert.rejects(verify('fixture', async () => new Response('', { status: 503 })));
  await assert.rejects(verify('fixture', async () => Response.json({ error: 'invalid' })));
});
test('verification accepts only same-origin token-authenticated results and closes listener', async () => {
  let address;
  const proof = await challenge({ region: 'test', prefix: 'test', sceneId: 'test' }, async url => {
    address = new URL(url);
    assert.equal((await fetch(url)).status, 200);
    const endpoint = new URL('/result' + address.search, address.origin);
    assert.equal((await fetch(endpoint, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: '{}' })).status, 403);
    assert.equal((await fetch(endpoint, { method: 'POST', headers: { Origin: 'http://other.example', 'Content-Type': 'application/json' }, body: '{}' })).status, 403);
    assert.equal((await fetch(endpoint, { method: 'POST', headers: { Origin: address.origin, 'Content-Type': 'application/json' }, body: JSON.stringify({ proof: 'fixture-proof' }) })).status, 200);
  }, 2000);
  assert.equal(proof, 'fixture-proof');
  await assert.rejects(fetch(address));
});
test('user cancellation, opening failure and timeout release pending verification', async () => {
  await assert.rejects(challenge({}, async url => {
    const origin = new URL(url).origin;
    await fetch(new URL('/result' + new URL(url).search, origin), {
      method: 'POST', headers: { Origin: origin, 'Content-Type': 'application/json' }, body: '{"cancelled":true}',
    });
  }, 1000));
  await assert.rejects(challenge({}, async () => { throw new Error('cannot open'); }, 1000));
  await assert.rejects(challenge({}, async () => {}, 10));
});
test('page escapes configuration and contains no account credential', () => {
  const html = page({ sceneId: '</script><script>bad' }, 'fixture-token');
  assert.ok(!html.includes('</script><script>bad'));
  assert.ok(!html.includes('requestAuth'));
});
