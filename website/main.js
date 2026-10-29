// dog.codes website — interactive demos and multilingual copy. No build step, no dependencies.
// English lives in index.html (data-i18n) and below (dyn, ph); every other language is a pack in
// ./i18n/<code>.js, loaded on demand: { name, short, htmlLang, ui, dyn, when, runs, os, ph }.
// Set these once the public repository exists.
const SITE = {
  github: 'https://github.com/owlfy-ai/dogcodes',
  releases: 'https://github.com/owlfy-ai/dogcodes/releases/latest',
};

const AGENTS = [
  ['Claude Code', 'claude-code.svg'], ['Codex', 'chatgpt.svg'], ['Grok Build', 'grok.svg'], ['Cursor', 'cursor.svg'],
  ['Antigravity', 'antigravity.svg'], ['OpenCode', 'opencode.png'], ['Pi', 'pi.svg'], ['Kiro', 'kiro.svg'],
  ['DeepSeek Harness', 'deepseek-harness.png'], ['ZCode', 'zcode.svg'], ['Kimi Code', 'kimi.svg'], ['CodeBuddy', 'codebuddy.svg'],
];
const icon = name => `assets/agents/${AGENTS.find(a => a[0] === name)[1]}`;
// Logos that are already a filled tile are shown as they are; the rest sit on a white tile.
const OWN_TILE = new Set(['Kimi Code', 'CodeBuddy', 'OpenCode', 'Kiro']);
const tone = name => (OWN_TILE.has(name) ? 'own-tile' : 'on-light');
// Optical size: thin or open marks are drawn larger so every logo reads at the same weight.
const SCALE = { 'Claude Code': 1.06, Codex: 1.2, 'Grok Build': 1.2, Cursor: 1.16, Antigravity: 1.12, Pi: 1.06, 'DeepSeek Harness': 1.22, ZCode: 1.13 };
const logo = name => `<img src="${icon(name)}" alt=""${SCALE[name] ? ` style="--s:${SCALE[name]}"` : ''}/>`;
const mini = name => `<span class="mini ${tone(name)}">${logo(name)}</span>`;
const reduced = matchMedia('(prefers-reduced-motion: reduce)').matches;
const $ = (s, el = document) => el.querySelector(s);
const sleep = ms => new Promise(r => setTimeout(r, reduced ? Math.min(ms, 60) : ms));

/* ---------------- Copy ---------------- */
const DYN = {
  en: {
    u1: 'Add rate limiting to the checkout API — 100 req/min per client.',
    c1: 'Plan: token bucket in Redis, keyed per client. Writing the middleware now.', c1t: '<b>edit</b> middleware/ratelimit.ts  +64 −3',
    h1: 'Claude Code quota exhausted → handed off to Codex · context included',
    x1: 'Picking up from the plan above. Middleware is in place — adding tests and wiring the config.', x1t: '<b>run</b> pnpm test  ✓ 42 passed',
    you: 'You', sel: n => `${n} selected`, findings: n => `${n} findings`, fixing: 'fixing…', fixed: '✓ fixed · ready to re-review',
    sent: (n, a) => `<b>→ Sent to Claude Code:</b> fix ${n} finding${n > 1 ? 's' : ''} from the Codex review. ${a}<br/>Review report attached.`,
    handTo: (name, turns) => `Handed off to ${name} · ${turns} turns of context`,
    start: ['Refactor the payment module into smaller services.', 'Split into three services — billing, refunds, ledger. Interfaces drafted in docs/payment.md.'],
    rounds: [
      ['Implement the billing service from that plan.', 'Read the plan from turn 2. Billing service implemented against the drafted interfaces — 14 tests added.'],
      ['Now extract refunds. They must stay idempotent.', 'Reused the ledger client from the last turn. Refunds extracted with a request-id guard.'],
      ['Wire all three into the gateway and run the suite.', 'Ledger extracted, all 3 services wired into the gateway. 118 tests passed — ready for review.'],
    ],
    reset: c => `resets in ${c}`, resetOn: w => `resets ${w}`, upgrade: 'Upgrade', upgraded: 'Updated', week: 'weekly', h5: '5-hour', month: 'monthly',
  },
};
let lang = 'en';
const D = () => DYN[lang];
const EN = new Map();
const EN_META = { title: document.title, desc: document.querySelector('meta[name="description"]').content };
const OS_EN = { mobile: 'Get the desktop app', desktop: n => `Download for ${n}` };
const WHEN_EN = ['Mon 09:00', 'Nov 4', 'Oct 23', 'Fri 22:00'];
const RUNS_EN = ['Dev server', 'Tests', 'Debug API', 'Build'];
const LANGS = [
  ['en', 'English', 'EN', 'en'], ['zh-CN', '简体中文', '简中', 'zh-CN'], ['zh-TW', '繁體中文', '繁中', 'zh-TW'], ['ja', '日本語', '日本語', 'ja'],
  ['ko', '한국어', '한국어', 'ko'], ['es', 'Español', 'ES', 'es'], ['fr', 'Français', 'FR', 'fr'], ['de', 'Deutsch', 'DE', 'de'], ['pt-BR', 'Português (BR)', 'PT', 'pt-BR'],
];
const packs = new Map();
let pack = { ui: {}, when: WHEN_EN, runs: RUNS_EN, os: OS_EN };
async function loadPack(code) {
  if (code === 'en') return { ui: {}, when: WHEN_EN, runs: RUNS_EN, os: OS_EN };
  if (!packs.has(code)) {
    const m = (await import(`./i18n/${code}.js`)).default;
    // Anything a pack leaves out falls back to English.
    packs.set(code, { ...m, dyn: { ...DYN.en, ...m.dyn }, ph: { ...PH.en, ...m.ph }, when: m.when || WHEN_EN, runs: m.runs || RUNS_EN, os: { ...OS_EN, ...m.os } });
  }
  return packs.get(code);
}
// Pick the best supported language for the browser's preference list.
function detectLanguage() {
  for (const l of navigator.languages?.length ? navigator.languages : [navigator.language || 'en']) {
    const t = l.toLowerCase();
    if (/^zh-(tw|hk|mo|hant)/.test(t)) return 'zh-TW';
    if (t.startsWith('zh')) return 'zh-CN';
    if (t === 'pt-br' || t.startsWith('pt')) return 'pt-BR';
    const base = t.split('-')[0];
    if (LANGS.some(x => x[0] === base)) return base;
    if (base === 'en') return 'en';
  }
  return 'en';
}
const COMMIT_LANG = { 'zh-CN': 'zh', 'zh-TW': 'zh', ja: 'ja' };

async function applyLanguage(next) {
  if (!LANGS.some(x => x[0] === next)) next = 'en';
  const loaded = await loadPack(next).catch(() => null);
  if (!loaded) { if (next !== 'en') return applyLanguage('en'); return; }
  pack = loaded; lang = next;
  if (!DYN[next]) DYN[next] = loaded.dyn;
  if (!PH[next]) PH[next] = loaded.ph;
  const entry = LANGS.find(x => x[0] === next);
  document.documentElement.lang = entry[3];
  const t = key => (next !== 'en' && pack.ui[key]) || EN.get(key);
  document.querySelectorAll('[data-i18n],[data-i18n-html]').forEach(el => {
    const key = el.dataset.i18n || el.dataset.i18nHtml;
    if (!EN.has(key)) EN.set(key, el.innerHTML);
    el.innerHTML = t(key);
  });
  document.title = (next !== 'en' && pack.ui['meta.title']) || EN_META.title;
  document.querySelector('meta[name="description"]').content = (next !== 'en' && pack.ui['meta.desc']) || EN_META.desc;
  $('#lang-cur').textContent = entry[2];
  document.querySelectorAll('#lang-list button').forEach(b => b.setAttribute('aria-checked', String(b.dataset.lang === next)));
  try { localStorage.setItem('dogcodes.site.lang', next); } catch { /* storage is optional */ }
  wireLinks(); osButton(); resetHandoff(); renderRemote(); renderQuota(); renderRuns(); countFindings();
  setCommitLanguage(COMMIT_LANG[next] || 'en');
}

function wireLinks() {
  document.querySelectorAll('[data-link="github"]').forEach(a => { a.href = SITE.github; a.target = '_blank'; a.rel = 'noopener'; });
  document.querySelectorAll('[data-link="release"]').forEach(a => { a.href = SITE.releases; a.target = '_blank'; a.rel = 'noopener'; });
}

function osButton() {
  const ua = navigator.userAgent;
  const label = $('#hero-download span');
  // Phones and tablets are not install targets: offer the platform cards instead.
  if (/Android|iPhone|iPad|iPod/i.test(ua)) {
    label.textContent = pack.os.mobile;
    $('#hero-download').href = '#download';
    return;
  }
  const os = /Windows/i.test(ua) ? 'win' : /Mac/i.test(ua) ? 'mac' : /Linux|X11/i.test(ua) ? 'linux' : 'mac';
  const names = { mac: 'macOS', win: 'Windows', linux: 'Linux' };
  label.textContent = pack.os.desktop(names[os]);
  $('#hero-download').href = SITE.releases;
  document.querySelectorAll('.dl a').forEach(a => a.classList.toggle('primary', a.dataset.os === os));
}

/* ---------------- Shared pieces ---------------- */
// The hero background: rows of agent logos drifting in alternating directions.
function buildAgents() {
  const tile = ([name]) => `<span class="bg-tile ${tone(name)}">${logo(name)}</span>`;
  $('#hero-bg').innerHTML = [0, 6, 3].map((shift, row) => {
    const order = [...AGENTS.slice(shift), ...AGENTS.slice(0, shift)].map(tile).join('');
    return `<div class="bg-row" style="--dur:${90 + row * 20}s;--dir:${row % 2 ? 'reverse' : 'normal'}"><div class="bg-track">${order}${order}</div></div>`;
  }).join('');
}

// The supported-agents section: one card per agent.
function buildAgentGrid() {
  $('#agent-grid').innerHTML = AGENTS.map(([name], i) => `<div class="agent-card spot reveal ${['', 'd1', 'd2', 'd3'][i % 4]}"><span class="ic ${tone(name)}">${logo(name)}</span><b>${name}</b></div>`).join('');
}

const message = (agent, text, tool = '') => `<div class="msg"><div class="av ${tone(agent)}">${logo(agent)}</div><div class="body"><div class="who"><b>${agent}</b></div><div class="text">${text}</div>${tool ? `<div class="tool">${tool}</div>` : ''}</div></div>`;
const userMessage = text => `<div class="msg user"><div class="body"><div class="text">${text}</div></div></div>`;
const divider = text => `<div class="handoff">${text}</div>`;

async function typeInto(el, text, speed = 14) {
  el.classList.add('caret');
  el.textContent = '';
  for (const ch of text) { el.textContent += ch; await sleep(speed); }
  el.classList.remove('caret');
}

/* ---------------- Hero loop ---------------- */
async function heroLoop() {
  const box = $('#hero-msgs');
  const setQuota = (id, pct, state) => {
    const el = $(id); el.querySelector('.bar i').style.setProperty('--w', `${pct}%`); el.querySelector('.pct').textContent = `${pct}%`;
    el.classList.toggle('empty', state === 'empty'); el.classList.toggle('active', state === 'active');
  };
  const chip = $('#hero-agent');
  for (;;) {
    const d = D();
    box.innerHTML = '';
    setQuota('#q-claude', 8, 'active'); setQuota('#q-codex', 76, '');
    chip.innerHTML = `${mini('Claude Code')}Claude Code <i class="ring-sm" style="--v:8"></i>`;
    await sleep(700);
    box.insertAdjacentHTML('beforeend', userMessage(d.u1));
    await sleep(900);
    box.insertAdjacentHTML('beforeend', message('Claude Code', '', d.c1t));
    await typeInto(box.lastElementChild.querySelector('.text'), d.c1);
    await sleep(900);
    setQuota('#q-claude', 0, 'empty');
    await sleep(800);
    box.insertAdjacentHTML('beforeend', divider(d.h1));
    setQuota('#q-codex', 76, 'active');
    chip.innerHTML = `${mini('Codex')}Codex <i class="ring-sm" style="--v:76"></i>`;
    await sleep(900);
    box.insertAdjacentHTML('beforeend', message('Codex', '', d.x1t));
    await typeInto(box.lastElementChild.querySelector('.text'), d.x1);
    await sleep(800);
    setQuota('#q-codex', 71, 'active');
    await sleep(4200);
  }
}

/* ---------------- Handoff demo ---------------- */
const HANDOFF = ['Claude Code', 'Codex', 'Grok Build', 'Antigravity'];
let current = 'Claude Code', step = 0, turns = 2, handing = false, pausedUntil = 0;
function resetHandoff() {
  const d = D();
  current = 'Claude Code'; step = 0; turns = 2;
  $('#handoff-feed').innerHTML = userMessage(d.start[0]) + message('Claude Code', d.start[1]);
  renderSwitch();
}
function renderSwitch() {
  $('#handoff-switch').innerHTML = HANDOFF.map(n => `<button type="button" data-a="${n}" aria-pressed="${n === current}">${mini(n)}${n}</button>`).join('');
}
async function handOff(name) {
  if (name === current || handing) return;
  handing = true;
  const d = D(), feed = $('#handoff-feed');
  current = name; renderSwitch();
  // Each round: switch the agent, then send the next prompt in the same conversation.
  const [prompt, reply] = d.rounds[step % d.rounds.length];
  feed.insertAdjacentHTML('beforeend', divider(d.handTo(name, turns)));
  await sleep(450);
  feed.insertAdjacentHTML('beforeend', userMessage(prompt));
  await sleep(700);
  feed.insertAdjacentHTML('beforeend', message(name, ''));
  await typeInto(feed.lastElementChild.querySelector('.text'), reply, 10);
  step += 1; turns += 2;
  while (feed.children.length > 12) feed.firstElementChild.remove();
  handing = false;
}
// Plays the handoff on its own while the demo is on screen; a click takes over for a while.
async function autoHandoff() {
  let visible = false;
  new IntersectionObserver(([e]) => { visible = e.isIntersecting; }, { threshold: 0.35 }).observe($('#handoff-feed'));
  for (;;) {
    await sleep(3200);
    if (!visible || handing || Date.now() < pausedUntil) continue;
    if (step >= D().rounds.length) { await sleep(1500); resetHandoff(); continue; }
    await handOff(HANDOFF[(HANDOFF.indexOf(current) + 1) % HANDOFF.length]);
  }
}

/* ---------------- Phone remote chapter ---------------- */
const PH = {
  en: {
    pc: 'MacBook · online',
    rail: [
      ['Pick up a PC conversation, or start a new one', 'Open any conversation from your computer and carry on — or start a fresh session: choose the agent, model and thinking effort.'],
      ['Approve actions, answer questions', 'Permission requests and agent questions arrive on your phone with a push notification. One tap and the agent keeps going.'],
      ['Review and commit from anywhere', 'See what changed across every project folder, stage files, let AI write the commit message, then commit and push.'],
      ['Create a project from your PC’s folders', 'Browse the directories on your computer and add one as a project — no need to sit back down at your desk.'],
      ['Run commands in your PC’s terminal', 'Open a terminal on your computer from the phone and run any command. It is the real shell on your machine.'],
      ['Run the project, see the web app on your phone', 'Start the dev server on your computer and open the running web app right on the phone — the real page, not a screenshot.'],
      ['Need the whole screen? Start remote desktop', 'Take control of your computer from the phone: tap, drag, scroll, type, and paste text or images.'],
    ],
    s1: { head: 'Sessions', rows: [['Claude Code', 'Add rate limiting', 'running'], ['Codex', 'Fix flaky payment test', '2 min ago'], ['Grok Build', 'Migrate to Postgres 17', '1 h ago']], agent: 'Agent', model: 'Model', effort: 'Effort', modelV: 'Opus', effortV: 'High', input: 'Describe the task…', go: 'Start session' },
    s2: { head: 'Add rate limiting', q: 'Which store should the rate limiter use?', opts: ['Redis', 'In-memory'], ap: 'Claude Code wants to run', deny: 'Deny', ok: 'Approve', done: '✓ Approved from your phone', reply: '42 tests passed. Ready for review.' },
    s3: { head: 'Changes', chip: '3 folders', msg: 'feat: split payment into billing, refunds, ledger', push: 'Commit & push', done: '✓ Pushed to origin/main' },
    s4: { head: 'Add project', path: 'MacBook › Workspace', repo: 'git repo', btn: 'Create project', ok: '✓ Project created' },
    s5: { head: 'Terminal', in: 'Type a command' },
    s6: { head: 'Run', running: 'running', h: 'Checkout', field: 'Card number', pay: 'Pay $49', paid: '✓ Payment received' },
    s7: { head: 'Remote desktop', tools: ['Keyboard', 'Right-click', '⌘ Cmd', 'Paste'] },
  },
};
const FOLDER = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/></svg>';
const head = (title, p) => `<div class="sc-head"><b>${title}</b><span class="sc-chip">${p.pc}</span></div>`;
function remScenes(p) {
  const a = p.s1, b = p.s2, g = p.s3, d = p.s4, t = p.s5, r = p.s6, k = p.s7;
  const files = [['M', 'src/payment/billing.ts', '+64', '−3'], ['A', 'src/payment/refunds.ts', '+88', '−0'], ['M', 'docs/payment.md', '+12', '−2']];
  return [
    `<div class="sc-body">${a.rows.map((x, i) => `<div class="s1-row${i === 0 ? ' hl' : ''}">${mini(x[0])}<div><b>${x[1]}</b><small>${x[0]}</small></div><em>${x[2]}</em></div>`).join('')}
      <div class="s1-sheet"><div class="s1-fields"><div><small>${a.agent}</small><b>Claude Code</b></div><div><small>${a.model}</small><b>${a.modelV}</b></div><div><small>${a.effort}</small><b>${a.effortV}</b></div></div><div class="s1-input">${a.input}</div><div class="btn-lime-s s1-go">${a.go}</div></div></div>`,
    `<div class="sc-body"><div class="sc-card s2-q"><p>${b.q}</p><div class="s2-opts"><span class="pick">${b.opts[0]}</span><span>${b.opts[1]}</span></div></div>
      <div class="sc-card s2-ap"><div>${b.ap}</div><code>pnpm test</code><div class="s2-btns"><span class="btn-ghost-s">${b.deny}</span><span class="btn-lime-s">${b.ok}</span></div></div>
      <div class="tool s2-done">${b.done}</div><div class="s2-reply">${message('Claude Code', b.reply)}</div></div>`,
    `<div class="sc-body">${files.map(f => `<div class="s3-file"><i></i><span>${f[1]}</span><u>${f[2]}</u><s>${f[3]}</s></div>`).join('')}
      <div class="sc-lbl">AI</div><div class="s3-msg"><span class="s3-type">${g.msg}</span></div><div style="flex:1"></div>
      <div class="s3-done">${g.done}</div><div class="btn-lime-s s3-push">${g.push}</div></div>`,
    `<div class="sc-body"><div class="s4-path">${d.path}</div>${['checkout-service', 'api-gateway', 'docs-site'].map((n, i) => `<div class="s4-dir${i === 0 ? ' sel' : ''}">${FOLDER}<b>${n}</b>${i < 2 ? `<small>${d.repo}</small>` : ''}</div>`).join('')}
      <div style="flex:1"></div><div class="s4-ok">${d.ok}</div><div class="btn-lime-s s4-create">${d.btn}</div></div>`,
    `<div class="sc-body"><div class="s5-term"><div><span class="p">$</span> git pull --rebase</div><div class="dim">Already up to date.</div><div><span class="p">$</span> pnpm test</div><div class="ok">✓ 42 tests passed</div><div><span class="p">$</span> pnpm build</div><div class="ok">✓ built in 1.8s</div></div><div class="s5-in">${t.in}<i>↑</i></div></div>`,
    `<div class="sc-body"><div class="s6-run"><svg viewBox="0 0 24 24" fill="currentColor"><path d="M7 4.5v15l12-7.5z"/></svg><b>dev server · :5173</b><em>${r.running}</em></div>
      <div class="s6-browser"><div class="s6-url">localhost:5173</div><div class="s6-page"><h4>${r.h}</h4><div class="s6-field">${r.field}</div><div class="s6-pay">${r.pay}</div><div class="s6-paid">${r.paid}</div></div></div></div>`,
    `<div class="sc-body"><div class="s7-screen"><div class="s7-menu"><i></i><span>Cursor</span><span>File</span><span>Edit</span></div>
      <div class="s7-win"><div class="bar"><i></i><i></i><i></i></div><div class="code"><span class="b" style="width:46%"></span><span style="width:78%"></span><span class="l" style="width:62%"></span><span style="width:54%"></span><span class="o" style="width:70%"></span><span style="width:38%"></span><span class="l" style="width:58%"></span></div></div>
      <div class="s7-dock"><i></i><i></i><i></i><i></i></div><div class="s7-ripple"></div>
      <svg class="s7-cursor" viewBox="0 0 18 18"><path d="M2 1.5v13l3.5-3.2 2.4 5.2 2.2-1-2.4-5.1H13z" fill="#fff" stroke="#000" stroke-width="1.2" stroke-linejoin="round"/></svg></div>
      <div class="s7-tools">${k.tools.map(x => `<span>${x}</span>`).join('')}</div></div>`,
  ].map((body, i) => `<div class="sc" data-i="${i}">${head(p['s' + (i + 1)].head, p)}${body}</div>`);
}
let remIdx = 0, remTimer = 0, remHoldUntil = 0, remVisible = false;
new IntersectionObserver(([e]) => { remVisible = e.isIntersecting; }, { threshold: 0.25 }).observe($('.phone-stage'));
function showScene(i) {
  remIdx = i;
  document.querySelectorAll('.sc').forEach(el => el.classList.toggle('on', Number(el.dataset.i) === i));
  document.querySelectorAll('.rem-item').forEach(el => el.classList.toggle('on', Number(el.dataset.i) === i));
  clearTimeout(remTimer);
  if (reduced) return;
  const next = () => {
    if (!remVisible || Date.now() < remHoldUntil) { remTimer = setTimeout(next, 1000); return; }
    showScene((remIdx + 1) % PH[lang].rail.length);
  };
  remTimer = setTimeout(next, 7500);
}
function renderRemote() {
  const p = PH[lang];
  $('#rem-rail').innerHTML = p.rail.map(([t, d], i) => `<button type="button" class="rem-item" data-i="${i}" data-n="${i + 1}"><h3>${t}</h3><p>${d}</p><i></i></button>`).join('');
  $('#rem-screens').innerHTML = remScenes(p).join('');
  $('#rem-rail').classList.remove('hold');
  showScene(Math.min(remIdx, p.rail.length - 1));
}
$('#rem-rail').addEventListener('click', e => {
  const b = e.target.closest('.rem-item'); if (!b) return;
  remHoldUntil = Date.now() + 25000; $('#rem-rail').classList.add('hold');
  showScene(Number(b.dataset.i));
});

/* ---------------- Quota grid ---------------- */
const QUOTA = [
  { a: 'Claude Code', p: 62, w: 'h5', secs: 2 * 3600 + 14 * 60 + 8, v: '2.1.301', nv: '2.1.304' },
  { a: 'Codex', p: 76, w: 'week', wi: 0, v: '0.47.0', nv: '0.48.1' },
  { a: 'Cursor', p: 86, w: 'month', wi: 1, v: '1.7.52' },
  { a: 'Kimi Code', p: 91, w: 'h5', secs: 4 * 3600 + 2 * 60 + 41, v: '0.9.3', nv: '0.9.5' },
  { a: 'Antigravity', p: 99, w: 'week', wi: 2, v: '1.12.0' },
  { a: 'ZCode', p: 0, w: 'week', wi: 3, v: '0.6.2' },
];
const clock = s => `${Math.floor(s / 3600)}h ${String(Math.floor(s / 60) % 60).padStart(2, '0')}m ${String(s % 60).padStart(2, '0')}s`;
const resetText = (q, d) => (q.secs != null ? d.reset(clock(q.secs)) : d.resetOn(pack.when[q.wi]));
function renderQuota() {
  const d = D();
  $('#quota-grid').innerHTML = QUOTA.map((q, i) => `<div class="card qcard spot reveal ${['', 'd1', 'd2'][i % 3]}" data-i="${i}">
    <div class="head"><span class="ic ${tone(q.a)}">${logo(q.a)}</span><b>${q.a}</b><small>v${q.v}</small>${q.nv ? `<button type="button" class="up" data-up="${i}">${d.upgrade} → ${q.nv}</button>` : ''}</div>
    <div class="ring" style="--c:${q.p < 10 ? '#ff6b5b' : 'var(--lime)'}"><span>0<small>%</small></span></div>
    <div class="meta"><b>${d[q.w]}</b><span class="rt">${resetText(q, d)}</span></div></div>`).join('');
  observe(document.querySelectorAll('#quota-grid .reveal'));
  document.querySelectorAll('#quota-grid .card').forEach(spotlight);
  // Re-rendered after a language switch: replay the rings if the grid is already on screen.
  if ($('#quota-grid').classList.contains('in')) animateRings();
}
function animateRings() {
  document.querySelectorAll('#quota-grid .qcard').forEach(card => {
    const q = QUOTA[card.dataset.i], ring = card.querySelector('.ring'), label = ring.querySelector('span');
    const t0 = performance.now(), dur = reduced ? 1 : 1400;
    const tick = now => {
      const k = Math.min(1, (now - t0) / dur), e = 1 - Math.pow(1 - k, 3), v = Math.round(q.p * e);
      ring.style.setProperty('--p', v); label.innerHTML = `${v}<small>%</small>`;
      if (k < 1) requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
  });
}
setInterval(() => {
  const d = D();
  QUOTA.forEach((q, i) => {
    if (q.secs == null) return;
    q.secs = Math.max(0, q.secs - 1);
    const el = document.querySelector(`#quota-grid [data-i="${i}"] .rt`);
    if (el) el.textContent = resetText(q, d);
  });
}, 1000);

/* ---------------- Review demo ---------------- */
function countFindings() {
  const boxes = [...document.querySelectorAll('#review .finding input')];
  const n = boxes.filter(b => b.checked && !b.closest('.finding').classList.contains('sent')).length;
  $('#review-count').textContent = D().sel(n);
  if (!$('#review-state').dataset.locked) $('#review-state').textContent = D().findings(boxes.length);
  $('#start-fix').disabled = n === 0;
}
async function startFix() {
  const d = D();
  const picked = [...document.querySelectorAll('#review .finding')].filter(f => f.querySelector('input').checked && !f.classList.contains('sent'));
  if (!picked.length) return;
  const summary = picked.map(f => `${f.querySelector('.sev').textContent}: ${f.querySelector('p span').textContent}`).join(' · ');
  picked.forEach((f, i) => setTimeout(() => f.classList.add('sent'), i * 120));
  const bubble = $('#fix-bubble');
  bubble.innerHTML = d.sent(picked.length, summary); bubble.classList.add('show');
  const state = $('#review-state'); state.dataset.locked = '1'; state.textContent = d.fixing;
  countFindings();
  await sleep(2200);
  state.textContent = d.fixed;
  await sleep(4500);
  document.querySelectorAll('#review .finding').forEach(f => f.classList.remove('sent'));
  bubble.classList.remove('show'); delete state.dataset.locked; countFindings();
}

/* ---------------- Git demo ---------------- */
const FILES = [
  ['M', 'middleware/', 'ratelimit.ts', '+64 −3', true], ['A', 'lib/', 'redis.ts', '+38', true],
  ['M', 'config/', 'limits.ts', '+6 −2', false], ['A', 'tests/', 'ratelimit.test.ts', '+20', true], ['D', 'legacy/', 'throttle.js', '−9', false],
];
const COMMIT = {
  en: 'feat(api): add Redis token-bucket rate limiting to checkout\n\n100 req/min per client; keyed by forwarded client IP.',
  zh: 'feat(api): 为结账接口添加基于 Redis 令牌桶的限流\n\n每个客户端每分钟 100 次，按转发后的客户端 IP 区分。',
  ja: 'feat(api): チェックアウト API に Redis トークンバケット方式のレート制限を追加\n\nクライアントごとに毎分 100 回、転送元 IP で識別。',
};
let msgLang = 'en';
// The commit message follows the page language until the visitor picks one.
function setCommitLanguage(l) {
  msgLang = l;
  document.querySelectorAll('#msg-lang button').forEach(x => x.setAttribute('aria-pressed', String(x.dataset.l === l)));
  if (!$('#commit-box .ph')) generateCommit();
}
function renderFiles() {
  $('#git-files').innerHTML = FILES.map((f, i) => `<div class="file ${f[4] ? 'staged' : ''}" data-f="${i}"><span class="box"></span><span class="k ${f[0]}">${f[0]}</span><span class="path"><small>${f[1]}</small>${f[2]}</span><span class="mono ${f[3].startsWith('−') ? 'del' : 'add'}" style="font-size:11.5px">${f[3]}</span></div>`).join('');
}
async function generateCommit() {
  const box = $('#commit-box');
  box.innerHTML = '<span></span>';
  const span = box.firstElementChild;
  span.style.whiteSpace = 'pre-wrap';
  await typeInto(span, COMMIT[msgLang], 12);
}

/* ---------------- Run demo ---------------- */
const RUNS = [
  { name: 'Dev server', cmd: 'pnpm dev', pin: true, out: ['<span class="dim">$ pnpm dev</span>', '  VITE v7.3.6  ready in 412 ms', '  ➜ Local:   <span class="url">http://localhost:5173/</span>', '<span class="ok">✓</span> api proxy → :8080', '<span class="dim">watching for changes…</span>'] },
  { name: 'Tests', cmd: 'pnpm test', out: ['<span class="dim">$ pnpm test</span>', ' ✓ ratelimit.test.ts (12)', ' ✓ checkout.test.ts (30)', ' Test Files  2 passed', '<span class="ok">✓ 42 passed</span> in 1.84s'] },
  { name: 'Debug API', cmd: 'go run ./cmd/api --debug', out: ['<span class="dim">$ go run ./cmd/api --debug</span>', 'debug: loading config/limits.yaml', 'debug: redis connected (2 ms)', '<span class="ok">listening</span> on <span class="url">:8080</span>', 'GET /checkout 200 3.1ms'] },
  { name: 'Build', cmd: 'pnpm build', out: ['<span class="dim">$ pnpm build</span>', 'transforming… 1,284 modules', 'dist/index.html      0.46 kB', 'dist/assets/index.js  212.4 kB │ gzip: 68.1 kB', '<span class="ok">✓ built in 4.71s</span>'] },
];
let runToken = 0;
function renderRuns() {
  $('#run-list').innerHTML = RUNS.map((r, i) => `<div class="run-item" data-r="${i}"><button class="play" type="button" aria-label="Run ${r.name}"><svg viewBox="0 0 16 16" fill="currentColor"><path d="M4 2.5v11l9-5.5-9-5.5Z"/></svg></button><div><b>${pack.runs[i]}</b><code>${r.cmd}</code></div>${r.pin ? '<span class="pin">📌</span>' : ''}</div>`).join('');
}
async function runEntry(i) {
  const token = ++runToken, term = $('#term');
  document.querySelectorAll('.run-item').forEach(el => el.classList.toggle('live', Number(el.dataset.r) === i));
  term.innerHTML = '';
  for (const line of RUNS[i].out) {
    if (token !== runToken) return;
    term.insertAdjacentHTML('beforeend', `<div>${line}</div>`);
    await sleep(380);
  }
}

/* ---------------- Per-turn usage ---------------- */
const TURNS = [[18, 52, 9, '41.2k'], [12, 66, 7, '58.9k'], [26, 41, 14, '37.5k'], [9, 74, 5, '64.1k'], [15, 60, 11, '52.3k']];
function renderTurns() {
  $('#turns').innerHTML = TURNS.map((t, i) => `<div class="turn"><span>#${i + 1}</span><div class="stack"><i class="in" data-w="${t[0]}"></i><i class="cache" data-w="${t[1]}"></i><i class="out" data-w="${t[2]}"></i></div><b>${t[3]}</b></div>`).join('');
}
const fillTurns = () => document.querySelectorAll('#turns .stack i').forEach(i => { i.style.width = `${i.dataset.w}%`; });

/* ---------------- Motion helpers ---------------- */
const io = new IntersectionObserver(entries => {
  for (const e of entries) {
    if (!e.isIntersecting) continue;
    e.target.classList.add('in');
    io.unobserve(e.target);
    if (e.target.id === 'quota-grid') animateRings();
    if (e.target.id === 'turns') fillTurns();
  }
}, { threshold: .18, rootMargin: '0px 0px -40px 0px' });
function observe(nodes) { nodes.forEach(n => io.observe(n)); }
function spotlight(el) {
  el.addEventListener('pointermove', e => {
    const r = el.getBoundingClientRect();
    el.style.setProperty('--mx', `${e.clientX - r.left}px`); el.style.setProperty('--my', `${e.clientY - r.top}px`);
  });
}

/* ---------------- Boot ---------------- */
buildAgents(); buildAgentGrid(); renderFiles(); renderTurns();
document.querySelectorAll('[data-i18n],[data-i18n-html]').forEach(el => EN.set(el.dataset.i18n || el.dataset.i18nHtml, el.innerHTML));
let saved = null;
try { saved = localStorage.getItem('dogcodes.site.lang'); } catch { /* optional */ }
if (saved === 'zh') saved = 'zh-CN';
await applyLanguage(saved || detectLanguage());

observe(document.querySelectorAll('.reveal'));
observe([$('#stage'), $('#fork-card'), $('#quota-grid'), $('#turns')]);
document.querySelectorAll('.spot').forEach(spotlight);

const nav = $('#nav');
const syncNav = () => nav.classList.toggle('scrolled', scrollY > 20);
addEventListener('scroll', syncNav, { passive: true });
syncNav(); // a restored scroll position must not leave the nav transparent
// Language menu: a button that opens a list of every supported language.
const langBtn = $('#lang-btn'), langList = $('#lang-list');
langList.innerHTML = LANGS.map(([code, name]) => `<li><button type="button" role="menuitemradio" data-lang="${code}" aria-checked="false" lang="${code}">${name}</button></li>`).join('');
langList.querySelector(`[data-lang="${lang}"]`)?.setAttribute('aria-checked', 'true');
const setMenu = open => { langList.hidden = !open; langBtn.setAttribute('aria-expanded', String(open)); };
langBtn.addEventListener('click', e => { e.stopPropagation(); setMenu(langList.hidden); });
langList.addEventListener('click', e => { const b = e.target.closest('button'); if (b) { setMenu(false); applyLanguage(b.dataset.lang); } });
document.addEventListener('click', () => setMenu(false));
document.addEventListener('keydown', e => { if (e.key === 'Escape') { setMenu(false); langBtn.focus(); } });
$('#handoff-switch').addEventListener('click', e => { const b = e.target.closest('button'); if (b) { pausedUntil = Date.now() + 12000; handOff(b.dataset.a); } });
$('#quota-grid').addEventListener('click', e => {
  const b = e.target.closest('[data-up]'); if (!b || b.classList.contains('done')) return;
  const q = QUOTA[b.dataset.up]; b.textContent = '…';
  setTimeout(() => { q.v = q.nv; b.classList.add('done'); b.textContent = `✓ ${D().upgraded}`; b.closest('.head').querySelector('small').textContent = `v${q.v}`; }, 900);
});
$('#review').addEventListener('change', countFindings);
$('#start-fix').addEventListener('click', startFix);
$('#git-files').addEventListener('click', e => { const f = e.target.closest('.file'); if (f) { FILES[f.dataset.f][4] = !FILES[f.dataset.f][4]; f.classList.toggle('staged'); } });
$('#gen-msg').addEventListener('click', generateCommit);
$('#msg-lang').addEventListener('click', e => {
  const b = e.target.closest('button'); if (!b) return;
  setCommitLanguage(b.dataset.l);
  generateCommit();
});
$('#run-list').addEventListener('click', e => { const it = e.target.closest('.run-item'); if (it) runEntry(Number(it.dataset.r)); });
$('#copy-cmd').addEventListener('click', async e => {
  try { await navigator.clipboard.writeText('pnpm install && pnpm desktop:dev'); e.target.textContent = '✓'; } catch { e.target.textContent = '⌘C'; }
});

heroLoop();
if (!reduced) autoHandoff();
// Show the run demo working without a click.
new IntersectionObserver((es, o) => { if (es[0].isIntersecting) { runEntry(0); o.disconnect(); } }, { threshold: .4 }).observe($('#run-list'));
new IntersectionObserver((es, o) => { if (es[0].isIntersecting) { generateCommit(); o.disconnect(); } }, { threshold: .5 }).observe($('#commit-box'));
