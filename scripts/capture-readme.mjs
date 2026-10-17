// Capture the real UI with the isolated, read-only documentation adapter.
import { mkdir } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const args = process.argv.slice(2);
function option(name, fallback) {
  const index = args.indexOf(name);
  if (index < 0) return fallback;
  if (!args[index + 1] || args[index + 1].startsWith('--')) throw new Error(`${name} requires a value`);
  return args[index + 1];
}
const base = new URL(option('--base-url', 'http://localhost:1420/'));
if (!['127.0.0.1', 'localhost', '[::1]'].includes(base.hostname)) throw new Error('Use a local Vite server for documentation screenshots.');
const output = path.resolve(option('--output', path.join(root, 'docs/images/readme')));
const modulePath = option('--playwright-module', null);
const { chromium } = await import(modulePath ? pathToFileURL(path.resolve(modulePath)).href : 'playwright');
const browser = await chromium.launch({ headless: true, channel: option('--browser', 'chrome') });
await mkdir(output, { recursive: true });
try {
  for (const scene of ['overview', 'switch-agent', 'run-project', 'review-fix', 'git-changes', 'projects', 'usage']) {
    const context = await browser.newContext({ viewport: { width: 1600, height: 1000 }, deviceScaleFactor: 1.5, locale: 'zh-CN', timezoneId: 'UTC', colorScheme: 'light', reducedMotion: 'reduce' });
    const page = await context.newPage();
    const failures = [];
    page.on('pageerror', error => failures.push(error.message));
    // No real provider, website or project service is contacted by the fixture.
    await context.route('**/*', route => {
      const url = new URL(route.request().url());
      return ['data:', 'blob:'].includes(url.protocol) || url.origin === base.origin ? route.continue() : route.abort();
    });
    await page.goto(new URL(`readme-fixture.html?scene=${scene}`, base).href);
    await page.getByRole('button', { name: '显示右侧边栏', exact: true }).waitFor();
    await page.getByText('已完成搜索与键盘导航。', { exact: true }).waitFor();
    if (['overview', 'run-project', 'review-fix', 'git-changes'].includes(scene)) {
      await page.getByRole('button', { name: '显示右侧边栏', exact: true }).click();
      await page.getByRole('button', { name: 'src/components/SearchBox.tsx', exact: true }).waitFor();
    }
    if (scene === 'run-project') {
      await page.getByRole('tab', { name: '运行', exact: true }).click();
      await page.getByText('构建生产版本', { exact: true }).waitFor();
      await page.locator('.xterm-screen').waitFor();
    } else if (scene === 'review-fix') {
      await page.getByRole('checkbox', { name: '选择问题：搜索应忽略首尾空格与大小写', exact: true }).check();
      await page.getByRole('button', { name: '开始修复', exact: true }).waitFor();
    } else if (scene === 'git-changes') {
      await page.getByRole('button', { name: 'src/components/SearchBox.tsx', exact: true }).click();
      await page.locator('.review-file-body .review-file-heading').waitFor();
    } else if (scene === 'switch-agent') {
      await page.locator('.composer .agent-picker > summary').click();
      await page.getByText('选择下一轮 Agent', { exact: true }).waitFor();
    } else if (scene === 'projects') {
      await page.locator('button.task').filter({ hasText: '添加笔记搜索和键盘导航' }).hover();
    }
    if (scene !== 'projects') await page.mouse.move(1599, 999);
    if (scene === 'usage') {
      await page.getByRole('button', { name: '本轮数据统计', exact: true }).first().hover();
      await page.getByRole('tooltip').waitFor();
    }
    await page.evaluate(() => document.fonts.ready);
    await page.waitForTimeout(800);
    if (failures.length) throw new Error(`${scene}: ${failures.join('; ')}`);
    const notices = await page.getByRole('alert').allTextContents();
    if (notices.length) throw new Error(`${scene}: unexpected alert: ${notices.join('; ')}`);
    await page.screenshot({ path: path.join(output, `${scene}.png`), animations: 'disabled' });
    console.log(`Captured ${scene}.png`);
    await context.close();
  }
} finally {
  await browser.close();
}
