// Start Plan uses the official interactive verification SDK. No account secret is sent to this page.
const http = require('node:http');
const { randomBytes } = require('node:crypto');
const { spawn } = require('node:child_process');
const SDK = 'https://o.alicdn.com/captcha-frontend/aliyunCaptcha/AliyunCaptcha.js';

function page(config, token) {
  const data = JSON.stringify({ config, token, sdk: SDK }).replaceAll('<', '\\u003c');
  return `<!doctype html><html lang="zh-CN"><meta charset="utf-8"><meta name="referrer" content="no-referrer">
<title>ZCode 账号验证</title><style>body{font:16px system-ui;max-width:520px;margin:10vh auto;padding:24px}button{font:inherit;padding:10px 20px;margin-right:12px}</style>
<h1>ZCode 账号验证</h1><p id="status">正在加载官方验证组件，完成后任务将继续。</p><div id="captcha"></div><button id="verify">继续验证</button><button id="cancel">取消</button>
<script>const {config,token,sdk}=${data};
let finished=false,instance,timer,requested=false;
const status=document.querySelector('#status');
async function finish(body){if(finished)return;finished=true;clearTimeout(timer);try{await fetch('/result?token='+token,{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)});status.textContent=body.proof?'验证完成，可以关闭此页。':'验证已取消或失败，可返回应用重试。';}catch{status.textContent='请求已结束，可以关闭此页。';}}
function interactive(){if(finished)return;status.textContent='请点击继续验证并完成官方验证，任务会自动继续。';document.querySelector('#verify').disabled=false;}
document.querySelector('#cancel').onclick=()=>finish({cancelled:true});
document.querySelector('#verify').onclick=()=>{if(!instance)status.textContent='验证组件仍在加载，请稍候。';};
window.AliyunCaptchaConfig={region:config.region,prefix:config.prefix};
const script=document.createElement('script');script.src=sdk;script.onerror=()=>finish({failed:true});
script.onload=()=>{const loaded=Date.now();try{window.initAliyunCaptcha({SceneId:config.sceneId,mode:'popup',language:'cn',element:'#captcha',button:'#verify',
getInstance(value){instance=value;if(requested)return;requested=true;document.querySelector('#verify').disabled=true;setTimeout(()=>{if(finished)return;try{value.startTracelessVerification();timer=setTimeout(interactive,8000);}catch{interactive();}},Math.max(0,loaded+2000-Date.now()));},
success(proof){if(typeof proof==='string'&&proof.trim())void finish({proof});},
fail(value){const code=value?.verifyCode??value?.VerifyCode;if((value?.success===true&&value?.verifyResult===true)||code==='T006'){const proof=value.captchaVerifyParam??value.CaptchaVerifyParam;if(typeof proof==='string'&&proof.trim())void finish({proof});}else if(value?.success===true&&value?.verifyResult===false)interactive();else void finish({failed:true});},
onError(){void finish({failed:true});}});}catch{void finish({failed:true});}};
document.head.append(script);
</script></html>`;
}

function openBrowser(url) {
  const [command, args] = process.platform === 'darwin' ? ['open', [url]]
    : process.platform === 'win32' ? ['rundll32.exe', ['url.dll,FileProtocolHandler', url]] : ['xdg-open', [url]];
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, { stdio: 'ignore', windowsHide: true });
    child.once('error', reject);
    child.once('exit', code => code === 0 ? resolve() : reject(new Error('verification_unavailable')));
  });
}

async function challenge(config, open = openBrowser, timeout = 130_000) {
  const token = randomBytes(32).toString('hex');
  let origin, timer, finish;
  const result = new Promise((resolve, reject) => {
    finish = (proof) => { clearTimeout(timer); proof ? resolve(proof) : reject(new Error('verification_unavailable')); };
  });
  // Opening may fail before the caller starts awaiting the result.
  void result.catch(() => {});
  const server = http.createServer(async (req, res) => {
    res.setHeader('Cache-Control', 'no-store'); res.setHeader('Referrer-Policy', 'no-referrer');
    res.setHeader('X-Content-Type-Options', 'nosniff'); res.setHeader('X-Frame-Options', 'DENY');
    const url = new URL(req.url || '/', origin);
    if (req.headers.host !== new URL(origin).host || url.searchParams.get('token') !== token) { res.writeHead(403).end(); return; }
    if (req.method === 'GET' && url.pathname === '/') {
      res.setHeader('Content-Type', 'text/html; charset=utf-8'); res.end(page(config, token)); return;
    }
    if (req.method !== 'POST' || url.pathname !== '/result' || req.headers.origin !== origin
      || req.headers['content-type'] !== 'application/json') { res.writeHead(403).end(); return; }
    try {
      const parts = []; let size = 0;
      for await (const part of req) { size += part.length; if (size > 65_536) throw new Error(); parts.push(part); }
      const body = JSON.parse(Buffer.concat(parts).toString('utf8'));
      const proof = typeof body.proof === 'string' && body.proof.trim() ? body.proof : undefined;
      res.end('{}'); finish(proof);
    } catch { res.writeHead(400).end(); }
  });
  try {
    await new Promise((resolve, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', resolve); });
    origin = `http://127.0.0.1:${server.address().port}`;
    timer = setTimeout(() => finish(), timeout);
    await Promise.race([open(`${origin}/?token=${token}`), result]);
    return await result;
  } finally {
    clearTimeout(timer);
    server.closeAllConnections();
    await new Promise(resolve => server.close(resolve));
  }
}

async function verify(version, fetcher = fetch, open = openBrowser) {
  const url = new URL('https://zcode.z.ai/api/v1/client/configs');
  url.searchParams.set('app_version', version); url.searchParams.set('platform', `${process.platform}-${process.arch}`);
  const response = await fetcher(url, { redirect: 'error', signal: AbortSignal.timeout(8000) });
  if (!response.ok) throw new Error('verification_unavailable');
  // The public configuration is bounded independently of the account response reader.
  const reader = response.body.getReader(); const chunks = []; let length = 0;
  try {
    while (true) { const { done, value } = await reader.read(); if (done) break;
      length += value.length; if (length > 65_536) throw new Error('verification_unavailable'); chunks.push(Buffer.from(value)); }
  } finally { await reader.cancel().catch(() => {}); }
  const configs = JSON.parse(Buffer.concat(chunks).toString('utf8'))?.data?.configs;
  if (!configs || typeof configs !== 'object') throw new Error('verification_unavailable');
  const config = configs.captcha;
  if (config?.enabled === false || config?.skip_model_request === true) return {};
  if (!['region', 'prefix', 'sceneId'].every(k => typeof config?.[k] === 'string' && config[k].trim())) return {};
  const proof = await challenge({ region: config.region, prefix: config.prefix, sceneId: config.sceneId }, open);
  return { 'X-Aliyun-Captcha-Verify-Param': proof, 'X-Aliyun-Captcha-Verify-Region': config.region };
}
module.exports = { verify, challenge, page };
