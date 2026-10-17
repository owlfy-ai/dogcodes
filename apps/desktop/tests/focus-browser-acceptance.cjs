// Run against Vite's read-only fixture, never a user workspace or Tauri instance.
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || 'playwright');
const assert = require('node:assert/strict');

(async () => {
  const browser = await chromium.launch({ headless: true, executablePath: process.env.REVIEW_CHROMIUM });
  try {
    const page = await browser.newPage({ viewport: { width: 1200, height: 900 } });
    await page.goto('http://127.0.0.1:1420/agent-picker-fixture.html');
    await page.evaluate(async () => {
      const { default: { createElement: h } } = await import('/node_modules/.vite/deps/react.js');
      const { default: { createRoot } } = await import('/node_modules/.vite/deps/react-dom_client.js');
      const { installFocusVisibility } = await import('/src/focusVisibility.ts');
      const { ProjectPicker } = await import('/src/ProjectPicker.tsx');
      const { ModelPicker } = await import('/src/ModelPicker.tsx');
      const { Tooltips } = await import('/src/Tooltips.tsx');
      await Promise.all(['/src/styles.css', '/src/sidebarIcons.css', '/src/review.css'].map(path => import(path)));
      installFocusVisibility(document);
      document.body.replaceChildren();
      const host = document.createElement('div');
      host.style.cssText = 'padding:40px; display:grid; gap:30px;';
      document.body.append(host);
      const project = { projectId: 'focus-test', displayName: 'Focus project', path: '/fixture', additionalDirectories: [], taskCount: 0 };
      createRoot(host).render(h('div', { style: { display: 'grid', gap: 24 } },
        h(ProjectPicker, { projects: [project], selectedProject: project, disabled: false, onSelect() {}, onOpenFolder() {} }),
        h(ModelPicker, { catalog: { models: [{ id: 'test', label: 'Test model', thinkingOptions: [] }], defaultModel: 'test' }, model: null, thinking: null, loading: false, error: null, disabled: false, onChange() {}, onRetry() {} }),
        ...['sidebar-window-controls', 'review-panel'].map(className => h('div', { className, key: className, style: { position: 'static', width: 300, height: 50, display: 'block', transform: 'none' } }, h('button', { 'aria-label': className, 'data-tooltip': 'Icon hint' }, 'Icon'))),
        h('details', null, h('summary', null, 'Details')),
        h('select', { 'aria-label': 'Native select' }, h('option', null, 'One'), h('option', null, 'Two')),
        h('input', { 'aria-label': 'Text entry', tabIndex: 0, style: { outline: '2px solid blue' } }),
        h(Tooltips)));
    });
    const ring = locator => locator.evaluate(node => getComputedStyle(node).outlineStyle);
    const project = page.getByRole('button', { name: '选择项目目录', exact: true });
    for (const theme of ['light', 'dark']) {
      await page.evaluate(theme => { document.documentElement.dataset.theme = theme; }, theme);
      await project.click();
      await page.getByRole('textbox', { name: '搜索工作区' }).fill('Focus');
      await page.keyboard.press('Escape');
      assert.equal(await project.evaluate(node => node === document.activeElement), true);
      assert.equal(await ring(project), 'none', `${theme}: pointer-opened search + Escape`);
      await project.click();
      await page.getByRole('button', { name: /Focus project/ }).click();
      assert.equal(await ring(project), 'none', `${theme}: pointer selection restores focus without ring`);
      const model = page.getByRole('button', { name: '选择模型', exact: true });
      await model.click();
      await page.getByRole('button', { name: 'Test model', exact: true }).click();
      assert.equal(await ring(model), 'none', `${theme}: model selection`);
      await model.click(); await page.keyboard.press('Escape');
      assert.equal(await ring(model), 'none', `${theme}: model Escape`);
      for (const name of ['sidebar-window-controls', 'review-panel']) {
        const button = page.getByRole('button', { name, exact: true });
        await button.click(); await page.keyboard.press('Escape');
        // Also exercise Safari-style explicit focus restoration.
        await button.focus();
        assert.equal(await ring(button), 'none', `${theme}: ${name} pointer ring`);
      }
      const select = page.getByRole('combobox', { name: 'Native select' });
      await select.dispatchEvent('pointerdown'); await select.selectOption({ label: 'Two' }); await select.focus();
      assert.equal(await ring(select), 'none', `${theme}: native select`);
      const input = page.getByRole('textbox', { name: 'Text entry' });
      await input.click();
      assert.equal(await ring(input), 'solid', `${theme}: text-entry feedback is preserved`);
      await project.click(); await page.keyboard.press('Escape');
      await page.keyboard.press('Tab');
      assert.equal(await model.evaluate(node => node === document.activeElement), true);
      assert.equal(await ring(model), 'solid', `${theme}: Tab focus stays visible`);
      await page.keyboard.press('Escape');
      assert.equal(await ring(model), 'none', `${theme}: Escape clears Tab highlighting without an open menu`);
      await page.keyboard.press('Tab');
      const sidebarIcon = page.getByRole('button', { name: 'sidebar-window-controls', exact: true });
      assert.equal(await sidebarIcon.evaluate(node => node === document.activeElement), true);
      assert.equal(await ring(sidebarIcon), 'solid', `${theme}: next Tab resumes navigation with visible focus`);
      await page.keyboard.press('Shift+Tab');
      await page.keyboard.press('Enter'); await page.keyboard.press('Escape');
      assert.equal(await ring(model), 'none', `${theme}: Escape clears keyboard-opened menu focus highlighting`);
      await project.click();
      await page.keyboard.press('ArrowDown'); await page.keyboard.press('Escape');
      assert.equal(await ring(project), 'none', `${theme}: Escape clears highlighting after arrow navigation`);
    }
    console.log('PASS: light/dark pointer selection, Escape, sidebar/review rings, native select, text entry, Tab/arrow navigation');
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exitCode = 1; });
