const {test} = require('node:test');
const assert = require('node:assert/strict');
const {windowsFrom, select, credential} = require('./quota-reader.cjs');
test('maps subscription windows and ignores tool quota', () => {
  const windows = windowsFrom({limits:[
    {type:'TIME_LIMIT',unit:5,number:1,percentage:1,nextResetTime:1791165601999},
    {type:'TOKENS_LIMIT',unit:3,number:5,percentage:0},
    {type:'TOKENS_LIMIT',unit:6,number:1,percentage:61,nextResetTime:1789264801996}
  ]});
  assert.equal(windows.find(w=>w.windowMinutes===300).usedPercent,0);
  assert.equal(windows.find(w=>w.windowMinutes===10080).resetsAtUnix,1789264801);
  assert.equal(windows.length, 2);
  assert.ok(windows.every(w => w.scope === null && !w.id.startsWith("TIME_LIMIT:")));
});
test('rejects unsupported hosts and does not pick another provider', () => {
  const config = {model:'selected/model',provider:{selected:{options:{baseURL:'https://evil.test',apiKey:'fixture'}}}};
  assert.throws(()=>select(config));
  config.provider.selected.options.baseURL='https://open.bigmodel.cn/api/anthropic';
  assert.equal(select(config).family,'BigModel');
  config.model='missing/model';assert.throws(()=>select(config));
});
test('rejects malformed cipher and percentages', () => {
  assert.throws(()=>credential('enc:v2:invalid'));
  assert.throws(()=>windowsFrom({limits:[{type:'TOKENS_LIMIT',unit:3,number:5,percentage:101}]}));
});
