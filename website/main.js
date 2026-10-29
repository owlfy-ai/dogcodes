// dog.codes website — interactive demos and EN/中文 copy. No build step, no dependencies.
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
const ZH = {
  'nav.features': '功能', 'nav.download': '下载', 'nav.get': '下载',
  'hero.badge': '开源', 'hero.eyebrow': '12 种 Coding Agent · 一个桌面工作台',
  'hero.title1': '把每一个', 'hero.title2': 'Coding Agent', 'hero.title3': '召集到一处。',
  'hero.lede': '同一个对话里，从 Claude Code 中途交给 Codex 接着做；所有额度一目了然；让一个 Agent 审查另一个；任何项目一键运行——用的都是你已经付费的订阅。',
  'hero.download': '下载 macOS 版', 'hero.star': '在 GitHub 上 Star', 'hero.meta1': '免费 · Apache-2.0', 'hero.meta3': '本地优先',
  'app.new': '新建对话', 'app.projects': '项目', 'app.t1': '添加接口限流', 'app.t2': '修复不稳定的支付测试', 'app.t3': '迁移到 Postgres 17', 'app.t4': '审查鉴权中间件',
  'app.ph': '让 Coding Agent 帮你……', 'app.quota': '额度', 'app.reset1': '2 小时 14 分后重置', 'app.reset2': '周一 09:00 重置', 'app.reset3': '4 小时 02 分后重置', 'app.reset4': '11 月 4 日重置',
  'agents.title': '<span class="nb">支持你已经在用的</span> <span class="nb">Coding Agent。</span>',
  'agents.sub': '已接入 12 种 Coding Agent，通过官方命令行工具和你现有的登录使用。可用模型、审批与用量数据因 Agent 而异。', 'agents.note': '更多 Agent 陆续接入中。',
  'handoff.title': '<span class="nb">换 Agent，</span><span class="nb">不换对话。</span>',
  'handoff.sub': '随时更换执行当前对话的 Agent，dog.codes 自动交接上下文——不用复制粘贴，也不用重新解释。',
  'handoff.p1': '把每项工作交给最适合的 Agent：一个负责设计，另一个补全细节。',
  'handoff.p2': '额度用完了？换另一个 Agent 接着做，无需重新说明需求。',
  'handoff.p3': '接手的 Agent 还能通过内置历史工具查阅之前的对话，是真正接着做。',
  'handoff.try': '试试看——选择下一个 Agent：',
  'quota.title': '<span class="nb">每份额度、</span><span class="nb">每次重置，</span><span class="nb">一眼看清。</span>',
  'quota.sub': '12 种 Agent 的剩余额度、重置时间与版本集中呈现。据此安排工作，把每一份 token 用足，任何 Agent 都能一键升级。',
  'acc.t1': '沿用你现有的订阅', 'acc.p1': 'dog.codes 调用你已登录的官方命令行工具——Claude Code、Codex、Cursor 等。无需额外账号，也没有额外的 API 账单。',
  'acc.t2': '自动发现', 'acc.p2': '启动时自动检测已安装的 Agent 及登录状态，未安装的提供官方安装入口。',
  'acc.t3': '本地优先', 'acc.p3': '数据留在你的电脑上，不经过中转服务器。Agent 启动时只拿到必要的环境变量，你的其他密钥不会外泄。',
  'review.title': '<span class="nb">让一个 Agent</span> <span class="nb">审查另一个。</span>',
  'review.sub': '指定另一个 Agent、另一个模型审查刚修改的代码。全新上下文，没有共同的盲点。也可做安全与性能分析。',
  'review.p1': '问题按高、中、低优先级分级。', 'review.p2': '选出要改的问题，一键开始修复。', 'review.p3': '修复交回主 Agent，并附上审查报告——上下文不断档。',
  'review.reviewing': '正在审查', 'review.fix': '开始修复 →',
  'review.f1': '限流键忽略了 X-Forwarded-For，所有经代理的客户端共用一个桶。',
  'review.f2': 'Redis 调用没有超时，一个慢节点会拖住所有请求。',
  'review.f3': '魔法数字 60 应改为具名常量。',
  'fork.title': '<span class="nb">分叉思路，</span><span class="nb">让 Agent 赛跑。</span>',
  'fork.sub': '从任意一轮分叉出新对话，交给另一个 Agent。原对话不受影响；worktree 让每种方案的代码彼此隔离。',
  'fork.p1': '让多个 Agent 尝试同一项任务。', 'fork.p2': '并排比较结果，保留最好的那一个。',
  'fork.l1': '第 1 轮', 'fork.l2': '第 2 轮', 'fork.l3': '从这里分叉', 'fork.a': '方案 A', 'fork.b': '方案 B', 'fork.keep': '✓ 采用这版',
  'git.title': '<span class="nb">Git，</span><span class="nb">就按你的习惯来。</span>',
  'git.sub': '高度贴近 VS Code 的侧边栏：暂存、取消暂存、丢弃、忽略、提交、提交并推送——树状或列表视图随意切换。',
  'git.p1': '指定任一 Agent 一键生成提交说明，并可选择语言。', 'git.ph': '提交说明……', 'git.gen': '用 Kimi 生成',
  'run.title': '<span class="nb">再也不用</span><span class="nb">记启动命令。</span>',
  'run.sub': 'dog.codes 分析每个项目的运行与调试方式，把命令和脚本变成可点击的启动项。',
  'run.p1': '不用再找终端——点一下启动，随时查看输出，用完就停。', 'run.p2': '常用项可以置顶，重新分析也会保留。', 'run.idle': '点击任意启动项的 ▶',
  'more.title': '<span class="nb">值得天天用的</span><span class="nb">每一处细节。</span>',
  'remote.title': '<span class="nb">Agent 在电脑上干活，</span><span class="nb">手机上随时指挥。</span>',
  'remote.sub': '用二维码配对一次手机。无论身在何处，都能发起会话、审批操作、回答 Agent 的提问——代码和 Agent 始终留在你的电脑上。',
  'remote.p1': '发起会话，选择 Agent、模型和思考强度，实时查看流式输出。',
  'remote.p2': '审批请求和 Agent 的提问都会送到手机，需要你时还会推送通知。',
  'remote.p3': '查看 Diff、提交推送，运行项目命令、打开终端，还能预览正在运行的网页应用。',
  'remote.c1t': '扫码一次，全程私密。', 'remote.c1p': '配对只需扫一次二维码，之后全程端到端加密。可配对多台电脑、查看在线状态，随时撤销某部手机。',
  'remote.c2t': '同样的会话，同样的 Agent。', 'remote.c2p': '手机只是看向你电脑上已有会话的一扇窗——历史、分叉、额度都在。代码和 Agent 始终留在你的电脑上。',
  'remote.c3t': '需要动手时，远程桌面。', 'remote.c3p': '任务需要真实屏幕时，直接在手机上查看电脑画面，点按、拖动、输入或粘贴文字和图片。',
  'more.t1': '多目录项目', 'more.p1': '前端、后端、文档分散在不同目录？一个会话全部交给 Agent——即便 Agent 本身不支持多目录。', 'more.tree': '三个目录，同一个会话',
  'more.t2': '关掉窗口，任务照跑。', 'more.p2': 'Agent、终端、项目命令都在后台服务中运行。重新打开即回到原处，进度分毫不差，每次提交和推送都只执行一次。', 'more.daemon': '仍在后台运行',
  'more.t3': '每一轮的用量明细', 'more.p3': '每一轮的 token 用量、上下文占用、缓存命中与缓存用量——清楚知道 token 花在了哪里。',
  'more.lin': '输入', 'more.lcache': '缓存命中', 'more.lout': '输出',
  'more.t4': '属于你，而不是我们', 'more.p4': 'Apache-2.0 开源。无需 dog.codes 账号。', 'more.k1': '数据留在你的电脑', 'more.k2': '沿用你现有的登录', 'more.k3': '每一行源码都可查看',
  'cta.title': '<span class="nb">把你的 Agent</span> <span class="nb">召集起来。</span>', 'cta.sub': '免费、开源、本地优先。带上你已有的 Agent 和订阅。', 'cta.copy': '复制',
  'cta.small': '也可以从源码构建——参见 <a data-link="github" href="#">README</a>。Agent 对话使用所选 Agent 的账号与额度。',
  'foot.fine': '© dog.codes contributors · Apache-2.0。文中产品名称、标志与商标归其各自所有者，仅用于标识兼容的工具，不代表任何背书。',
};
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
    ph: {
      pc: 'MacBook · online', u: 'Add rate limiting to the checkout API.', c: 'Plan: token bucket in Redis, per client. Writing the middleware.',
      toastT: 'Approval needed', toastS: 'Claude Code wants to run pnpm test', q: 'Claude Code wants to run a command on your computer.',
      deny: 'Deny', ok: 'Approve', approved: '✓ Approved from your phone', done: '42 tests passed. Rate limiting is ready for review.', hint: 'Message Claude Code…',
    },
    reset: 'resets in', resetOn: 'resets', upgrade: 'Upgrade', upgraded: 'Updated', week: 'weekly', h5: '5-hour', month: 'monthly',
  },
  zh: {
    u1: '给结账接口加限流——每个客户端每分钟 100 次。',
    c1: '方案：在 Redis 中用令牌桶，按客户端区分。开始编写中间件。', c1t: '<b>编辑</b> middleware/ratelimit.ts  +64 −3',
    h1: 'Claude Code 额度用尽 → 已交给 Codex 接续 · 已附上下文',
    x1: '接着上面的方案继续。中间件已就位——现在补测试并接入配置。', x1t: '<b>运行</b> pnpm test  ✓ 42 项通过',
    you: '你', sel: n => `已选 ${n} 项`, findings: n => `${n} 个问题`, fixing: '修复中…', fixed: '✓ 已修复 · 可重新审查',
    sent: (n, a) => `<b>→ 已发送给 Claude Code：</b>修复 Codex 审查发现的 ${n} 个问题。${a}<br/>已附审查报告。`,
    handTo: (name, turns) => `已交给 ${name} · 附 ${turns} 轮上下文`,
    start: ['把支付模块拆分成更小的服务。', '拆成三个服务——计费、退款、账本。接口草案已写在 docs/payment.md。'],
    rounds: [
      ['按这个方案先实现计费服务。', '已读取第 2 轮的方案。计费服务已按起草的接口实现，新增 14 个测试。'],
      ['接着拆出退款服务，必须保持幂等。', '复用了上一轮的账本客户端。退款服务已拆出，并加上请求 ID 校验。'],
      ['把三个服务接入网关，跑一遍完整测试。', '账本已拆出，3 个服务都已接入网关。118 项测试通过，可以进入审查。'],
    ],
    ph: {
      pc: 'MacBook · 在线', u: '给结账接口加限流。', c: '方案：在 Redis 中用令牌桶，按客户端区分。开始编写中间件。',
      toastT: '需要审批', toastS: 'Claude Code 想运行 pnpm test', q: 'Claude Code 想在你的电脑上运行一条命令。',
      deny: '拒绝', ok: '批准', approved: '✓ 已在手机上批准', done: '42 项测试通过，限流已可进入审查。', hint: '给 Claude Code 发消息……',
    },
    reset: '后重置', resetOn: '重置', upgrade: '升级', upgraded: '已更新', week: '每周', h5: '5 小时', month: '每月',
  },
};
let lang = 'en';
const D = () => DYN[lang];
const EN = new Map();

function applyLanguage(next) {
  lang = next;
  document.documentElement.lang = next === 'zh' ? 'zh-CN' : 'en';
  document.querySelectorAll('[data-i18n]').forEach(el => {
    const key = el.dataset.i18n;
    if (!EN.has(key)) EN.set(key, el.innerHTML);
    el.innerHTML = next === 'zh' && ZH[key] ? ZH[key] : EN.get(key);
  });
  document.querySelectorAll('[data-i18n-html]').forEach(el => {
    const key = el.dataset.i18nHtml;
    if (!EN.has(key)) EN.set(key, el.innerHTML);
    el.innerHTML = next === 'zh' && ZH[key] ? ZH[key] : EN.get(key);
  });
  document.querySelectorAll('.lang button').forEach(b => b.setAttribute('aria-pressed', String(b.dataset.lang === next)));
  try { localStorage.setItem('dogcodes.site.lang', next); } catch { /* storage is optional */ }
  wireLinks(); osButton(); resetHandoff(); phoneLoop(); renderQuota(); renderRuns(); countFindings();
  setCommitLanguage(next === 'zh' ? 'zh' : 'en');
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
    label.textContent = lang === 'zh' ? '下载桌面版' : 'Get the desktop app';
    $('#hero-download').href = '#download';
    return;
  }
  const os = /Windows/i.test(ua) ? 'win' : /Mac/i.test(ua) ? 'mac' : /Linux|X11/i.test(ua) ? 'linux' : 'mac';
  const names = { mac: 'macOS', win: 'Windows', linux: 'Linux' };
  label.textContent = lang === 'zh' ? `下载 ${names[os]} 版` : `Download for ${names[os]}`;
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

/* ---------------- Phone remote demo ---------------- */
let phoneRun = 0, phoneVisible = false;
new IntersectionObserver(([e]) => { phoneVisible = e.isIntersecting; }, { threshold: 0.3 }).observe($('.phone'));
async function phoneLoop() {
  const run = ++phoneRun, alive = () => run === phoneRun;
  const d = D().ph, feed = $('#ph-feed'), approve = $('#ph-approve'), toast = $('#ph-toast'), ok = $('#ph-ok');
  const paint = () => {
    feed.innerHTML = ''; approve.classList.remove('show'); toast.classList.remove('show');
    $('#ph-pc').textContent = d.pc; $('#ph-hint').textContent = d.hint;
    $('#ph-toast-t').textContent = d.toastT; $('#ph-toast-s').textContent = d.toastS;
    $('#ph-ap-q').textContent = d.q; $('#ph-deny').textContent = d.deny; ok.textContent = d.ok;
  };
  paint();
  if (reduced) {
    feed.insertAdjacentHTML('beforeend', userMessage(d.u) + message('Claude Code', d.c) + `<div class="tool">${d.approved}</div>` + message('Claude Code', d.done));
    return;
  }
  while (alive()) {
    while (!phoneVisible && alive()) await sleep(400);
    if (!alive()) return;
    paint();
    await sleep(700);
    feed.insertAdjacentHTML('beforeend', userMessage(d.u));
    await sleep(900);
    feed.insertAdjacentHTML('beforeend', message('Claude Code', ''));
    await typeInto(feed.lastElementChild.querySelector('.text'), d.c, 16);
    await sleep(800);
    if (!alive()) return;
    toast.classList.add('show');
    await sleep(1700);
    toast.classList.remove('show'); approve.classList.add('show');
    await sleep(2300);
    if (!alive()) return;
    ok.classList.add('tap');
    await sleep(320);
    ok.classList.remove('tap'); approve.classList.remove('show');
    feed.insertAdjacentHTML('beforeend', `<div class="tool">${d.approved}</div>`);
    await sleep(900);
    feed.insertAdjacentHTML('beforeend', message('Claude Code', ''));
    await typeInto(feed.lastElementChild.querySelector('.text'), d.done, 16);
    await sleep(4800);
    while (feed.children.length > 6) feed.firstElementChild.remove();
  }
}

/* ---------------- Quota grid ---------------- */
const QUOTA = [
  { a: 'Claude Code', p: 62, w: 'h5', secs: 2 * 3600 + 14 * 60 + 8, v: '2.1.301', nv: '2.1.304' },
  { a: 'Codex', p: 76, w: 'week', on: 'Mon 09:00', onZh: '周一 09:00', v: '0.47.0', nv: '0.48.1' },
  { a: 'Cursor', p: 86, w: 'month', on: 'Nov 4', onZh: '11 月 4 日', v: '1.7.52' },
  { a: 'Kimi Code', p: 91, w: 'h5', secs: 4 * 3600 + 2 * 60 + 41, v: '0.9.3', nv: '0.9.5' },
  { a: 'Antigravity', p: 99, w: 'week', on: 'Oct 23', onZh: '10 月 23 日', v: '1.12.0' },
  { a: 'ZCode', p: 0, w: 'week', on: 'Fri 22:00', onZh: '周五 22:00', v: '0.6.2' },
];
const clock = s => `${Math.floor(s / 3600)}h ${String(Math.floor(s / 60) % 60).padStart(2, '0')}m ${String(s % 60).padStart(2, '0')}s`;
const resetText = (q, d) => q.secs != null ? (lang === 'zh' ? `${clock(q.secs)} ${d.reset}` : `${d.reset} ${clock(q.secs)}`) : (lang === 'zh' ? `${q.onZh} ${d.resetOn}` : `${d.resetOn} ${q.on}`);
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
  { name: 'Dev server', zh: '开发服务器', cmd: 'pnpm dev', pin: true, out: ['<span class="dim">$ pnpm dev</span>', '  VITE v7.3.6  ready in 412 ms', '  ➜ Local:   <span class="url">http://localhost:5173/</span>', '<span class="ok">✓</span> api proxy → :8080', '<span class="dim">watching for changes…</span>'] },
  { name: 'Tests', zh: '运行测试', cmd: 'pnpm test', out: ['<span class="dim">$ pnpm test</span>', ' ✓ ratelimit.test.ts (12)', ' ✓ checkout.test.ts (30)', ' Test Files  2 passed', '<span class="ok">✓ 42 passed</span> in 1.84s'] },
  { name: 'Debug API', zh: '调试 API', cmd: 'go run ./cmd/api --debug', out: ['<span class="dim">$ go run ./cmd/api --debug</span>', 'debug: loading config/limits.yaml', 'debug: redis connected (2 ms)', '<span class="ok">listening</span> on <span class="url">:8080</span>', 'GET /checkout 200 3.1ms'] },
  { name: 'Build', zh: '构建', cmd: 'pnpm build', out: ['<span class="dim">$ pnpm build</span>', 'transforming… 1,284 modules', 'dist/index.html      0.46 kB', 'dist/assets/index.js  212.4 kB │ gzip: 68.1 kB', '<span class="ok">✓ built in 4.71s</span>'] },
];
let runToken = 0;
function renderRuns() {
  $('#run-list').innerHTML = RUNS.map((r, i) => `<div class="run-item" data-r="${i}"><button class="play" type="button" aria-label="Run ${r.name}"><svg viewBox="0 0 16 16" fill="currentColor"><path d="M4 2.5v11l9-5.5-9-5.5Z"/></svg></button><div><b>${lang === 'zh' ? r.zh : r.name}</b><code>${r.cmd}</code></div>${r.pin ? '<span class="pin">📌</span>' : ''}</div>`).join('');
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
applyLanguage(saved || (/^zh/i.test(navigator.language) ? 'zh' : 'en'));

observe(document.querySelectorAll('.reveal'));
observe([$('#stage'), $('#fork-card'), $('#quota-grid'), $('#turns')]);
document.querySelectorAll('.spot').forEach(spotlight);

const nav = $('#nav');
const syncNav = () => nav.classList.toggle('scrolled', scrollY > 20);
addEventListener('scroll', syncNav, { passive: true });
syncNav(); // a restored scroll position must not leave the nav transparent
document.querySelectorAll('.lang button').forEach(b => b.addEventListener('click', () => applyLanguage(b.dataset.lang)));
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
