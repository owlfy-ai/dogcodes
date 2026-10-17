const {test} = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const context = {require, module:{exports:{}}, Buffer, URL, process, console};
vm.runInNewContext(fs.readFileSync(__dirname+'/quota-reader.cjs','utf8')+'\n'+fs.readFileSync(__dirname+'/runtime-reader.cjs','utf8'),context);
const build = context.module.exports.runtimeFrom;
const config = {provider:{one:{kind:'anthropic',options:{apiKey:'fixture-key',baseURL:'https://example.test/v1',headers:{'x-test':'fixture'}},models:{model:{}}}}};
const input = {model:{providerId:'one',modelId:'model',variant:'high'},thoughtLevel:'high',definition:{contextWindow:200000}};
test('keeps exact model, thought, endpoint and native credential in runtime payload',()=>{
 const value=build(config,input);
 assert.equal(value.model.providerId,'one'); assert.equal(value.model.variant,'high');
 assert.equal(value.thoughtLevel,'high'); assert.equal(value.provider.kind,'anthropic');
 assert.equal(value.provider.apiKey.value,'fixture-key'); assert.equal(value.provider.baseURL,'https://example.test/v1');
 assert.equal(value.provider.models[0].contextWindow,200000);
 assert.equal(config.provider.one.options.apiKey,'fixture-key');
});
test('never borrows another provider or falls back to another model',()=>{
 for(const model of [{providerId:'absent',modelId:'model'},{providerId:'one',modelId:'absent'}]) assert.throws(()=>build(config,{...input,model}));
 assert.throws(()=>build({provider:{one:{...config.provider.one,enabled:false}}},input));
});
test('rejects unsupported credential and endpoint formats',()=>{
 assert.throws(()=>build({provider:{one:{...config.provider.one,options:{apiKey:'enc:v2:unknown'}}}},input));
 assert.throws(()=>build({provider:{one:{...config.provider.one,options:{baseURL:'file:///tmp/key'}}}},input));
});
