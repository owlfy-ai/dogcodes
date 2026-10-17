// Run against the read-only fixture, never a user workspace or Tauri instance.
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || 'playwright');
const assert = require('node:assert/strict');

(async () => {
  const browser = await chromium.launch({ headless: true, executablePath: process.env.REVIEW_CHROMIUM });
  try {
    const page = await browser.newPage({ viewport: { width: 1440, height: 1000 } });
    await page.goto('http://127.0.0.1:1420/review-fixture.html');
    await page.getByRole('navigation', { name: 'Review 文件列表' }).locator('.review-file-entry').last().click();
    await page.locator('.review-file-entry').last().click({ button: 'right' });
    await page.getByRole('menuitem', { name: '丢弃更改…', exact: true }).click();
    const dialog = page.getByRole('dialog', { name: '丢弃文件更改' });
    await dialog.waitFor({ state: 'visible' });
    assert(await dialog.evaluate(element => element.matches(':modal')));
    const background = page.locator('#background-action');
    let blocked = false;
    try { await background.click({ timeout: 500 }); } catch { blocked = true; }
    assert(blocked, 'modal must block background actions');
    assert.equal(await background.textContent(), '背后操作次数：0');
    await dialog.getByRole('button', { name: '丢弃更改', exact: true }).click();
    await dialog.getByRole('alert').waitFor();
    assert.match(await dialog.getByRole('alert').textContent(), /没有修改磁盘/);
    await page.screenshot({ path: process.argv[2] || '/tmp/dogcodes-review-modal.png' });
    await page.keyboard.press('Escape');
    await dialog.waitFor({ state: 'hidden' });
    assert(await page.evaluate(() => document.activeElement.classList.contains('review-file-entry')));
    await page.getByRole('button', { name: '左右对照', exact: true }).click();
    assert(await page.locator('code').count() > 0);
    await page.goto('http://127.0.0.1:1420/review-fixture.html?rename=1');
    const tree = page.getByRole('navigation', { name: 'Review 文件列表' });
    const renamePath = 'packages/very-long-project-name/src/features/review/ConversationReview.tsx';
    const entry = tree.getByRole('button', { name: renamePath, exact: true });
    await entry.waitFor();
    assert.match(await entry.getAttribute("data-tooltip"), /legacy\/OriginalReview.tsx/);
    assert.equal(await tree.getByRole('region', { name: '未跟踪文件', exact: true }).count(), 0);
    await page.getByRole('button', { name: '以树形显示', exact: true }).click();
    const folder = tree.locator('.review-folder[data-tooltip="packages"]');
    await folder.click();
    assert.equal(await entry.count(), 0, 'folded folder hides its descendants');
    await page.getByLabel('筛选文件', { exact: true }).fill('OriginalReview');
    await entry.waitFor();
    await entry.click();
    await page.locator('.review-file-entry').last().click({ button: 'right' });
    await page.getByRole('menuitem', { name: '丢弃更改…', exact: true }).click();
    await dialog.waitFor();
    assert.match(await dialog.textContent(), /legacy\/OriginalReview.tsx/);
    assert.match(await dialog.textContent(), /ConversationReview.tsx/);
    await page.keyboard.press('Escape');
    await dialog.waitFor({ state: 'hidden' });
    await page.getByLabel('筛选文件', { exact: true }).fill('');
    assert.equal(await entry.count(), 0, 'filtering preserves the earlier folder fold state');
    await folder.click();
    await entry.waitFor();
    await page.getByRole('button', { name: '切换主题', exact: true }).click();
    await page.screenshot({ path: '/tmp/dogcodes-review-rename-light.png' });
    console.log('PASS: Chromium modal isolation, error rendering, Escape/focus restore, split view, compact changes tree');
    console.log('PASS: rename grouping, source-path filtering, preserved folds, paired confirmation and light-theme capture');
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exitCode = 1; });
