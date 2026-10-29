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
  'remote.title': '<span class="nb">出门不必带电脑，</span><span class="nb">Agent 照常干活。</span>',
  'remote.sub': '电脑在家开着，Agent 照常运行。你在外面，用手机就能接续对话、批准操作、提交代码，必要时还能直接接管整块屏幕。',
  'remote.note': '电脑需保持开机，dog.codes 保持打开（最小化也可以）。',
  'remote.lab1': '在家，持续工作', 'remote.lab2': '握在手里',
  'remote.st1': '扫一次二维码', 'remote.st2': '离开办公桌', 'remote.st3': '随时随地指挥 Agent',
  'remote.c1t': '扫码一次，全程私密。', 'remote.c1p': '配对只需扫一次二维码，之后全程端到端加密。可配对多台电脑、查看在线状态，随时撤销某部手机。',
  'remote.c2t': '同样的会话，同样的 Agent。', 'remote.c2p': '手机只是看向你电脑上已有会话的一扇窗——历史、分叉、额度都在。代码和 Agent 始终留在你的电脑上。',
  'remote.c4t': '审批权始终在你手里。', 'remote.c4p': 'Agent 仍然会先询问再动手。每个请求在你批准后，都会先与电脑上原本的运行核对，才会生效。',
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
  wireLinks(); osButton(); resetHandoff(); renderRemote(); renderQuota(); renderRuns(); countFindings();
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
  zh: {
    pc: 'MacBook · 在线',
    rail: [
      ['接续电脑上的对话，或发起新对话', '打开电脑上的任意对话继续聊，也可以新建会话：选择 Agent、模型和思考强度。'],
      ['批准权限请求，回答 Agent 的问题', '权限请求和 Agent 的提问会推送到手机。点一下批准或作答，Agent 立刻继续。'],
      ['随时随地审查、提交代码', '查看项目各个目录的改动，勾选暂存，让 AI 写好提交说明，再提交并推送。'],
      ['直接选电脑里的目录创建项目', '浏览电脑上的目录，选一个就能创建项目，不必坐回桌前。'],
      ['在电脑终端里执行命令', '在手机上打开电脑的终端，执行任意命令——跑的就是你电脑上的真实 Shell。'],
      ['运行项目，在手机上看网页效果', '在电脑上启动开发服务，手机里直接打开正在运行的网页应用——看到的就是真实页面，而不是截图。'],
      ['需要整块屏幕？开启远程桌面', '在手机上接管电脑：点按、拖动、滚动、输入，还能粘贴文字和图片。'],
    ],
    s1: { head: '会话', rows: [['Claude Code', '添加接口限流', '运行中'], ['Codex', '修复不稳定的支付测试', '2 分钟前'], ['Grok Build', '迁移到 Postgres 17', '1 小时前']], agent: 'Agent', model: '模型', effort: '强度', modelV: 'Opus', effortV: '高', input: '描述任务……', go: '开始会话' },
    s2: { head: '添加接口限流', q: '限流器应该使用哪种存储？', opts: ['Redis', '内存'], ap: 'Claude Code 想运行', deny: '拒绝', ok: '批准', done: '✓ 已在手机上批准', reply: '42 项测试通过，可以进入审查。' },
    s3: { head: '改动', chip: '3 个目录', msg: 'feat: 把支付拆分为计费、退款、账本', push: '提交并推送', done: '✓ 已推送到 origin/main' },
    s4: { head: '添加项目', path: 'MacBook › Workspace', repo: 'Git 仓库', btn: '创建项目', ok: '✓ 项目已创建' },
    s5: { head: '终端', in: '输入命令' },
    s6: { head: '运行', running: '运行中', h: '结账', field: '卡号', pay: '支付 ¥49', paid: '✓ 支付成功' },
    s7: { head: '远程桌面', tools: ['键盘', '右键', '⌘ 键', '粘贴'] },
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
  const p = PH[lang === 'zh' ? 'zh' : 'en'];
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
