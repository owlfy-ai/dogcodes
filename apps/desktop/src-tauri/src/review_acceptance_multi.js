(async () => {
  const invoke = (...args) => window.__TAURI_INTERNALS__.invoke(...args);
  const checks = [];
  const assert = (value, message) => { if (!value) throw new Error(message); };
  const waitFor = async (read, label) => {
    const deadline = Date.now() + 20000;
    while (Date.now() < deadline) {
      const value = await read(); if (value) return value;
      await new Promise(resolve => setTimeout(resolve, 50));
    }
    throw new Error(`${label}: ${[...document.querySelectorAll('[role="alert"]')].map(node => node.textContent).join(" | ")}`);
  };
  let error = null;
  try {
    const bootstrap = await invoke("core_bootstrap");
    const task = bootstrap.tasks.find(task => task.title === "多目录 Diff 验收");
    assert(task, "multi-directory task missing");
    const taskId = task.taskId;
    const project = bootstrap.projects.find(project => project.projectId === task.projectId);
    assert(project.additionalDirectories.length === 2, "project must contain three explicit roots");
    const snapshot = await invoke("core_review_query", { query: { taskId, mode: "uncommitted" } });
    assert(snapshot.roots.length === 3 && snapshot.roots.filter(root => root.root.git).length === 2, "mixed root discovery mismatch");
    for (const [index, name] of ["root-a", "root-b", "root-notes"].entries()) {
      const root = snapshot.roots.find(root => root.root.path.endsWith(`/${name}`));
      assert(root && !root.error && root.files.length === 1, "root contents mixed or missing");
      const file = await invoke("core_review_file", { taskId, snapshotId: snapshot.id, rootId: root.root.id, path: "shared.txt" });
      assert(file.text.before.join("").includes(`base root ${index}`) && file.text.after.join("").includes(`changed root ${index}`), "same-named files crossed root boundaries");
    }
    checks.push("three explicit project roots expose isolated same-named file diffs");
    (await waitFor(() => document.querySelector('button.task[data-tooltip="多目录 Diff 验收"]'), "task row")).click();
    const button = label => [...document.querySelectorAll("button")].find(node => node.textContent === label && !node.disabled);
    (await waitFor(() => button("Diff"), "Diff control")).click();
    const panel = () => document.querySelector('[aria-label="工作区 Review"]');
    (await waitFor(() => panel()?.querySelector('button[aria-label="View as Tree"]'), "tree toggle")).click();
    await waitFor(() => panel()?.querySelectorAll('.review-root-heading').length === 3, "three directory headings");
    for (const [index, name] of ["root-a", "root-b", "root-notes"].entries()) {
      const root = snapshot.roots.find(root => root.root.path.endsWith(`/${name}`));
      const file = await waitFor(() => panel()?.querySelector(`[data-root-id="${root.root.id}"] button[aria-label="shared.txt"]:not(:disabled)`), "root file selection");
      file.click();
      await waitFor(() => panel()?.querySelector(".review-file-body")?.textContent.includes(`changed root ${index}`), "correct root body");
      assert(Boolean(panel().querySelector('select[aria-label="比较模式"]')) === (index < 2), "Git mode leaked into non-Git directory");
      checks.push(`native root switch displays ${name} with correct comparison mode`);
    }
    await waitFor(() => panel()?.querySelectorAll('button[aria-label="shared.txt"]').length === 3, "all roots file groups");
    checks.push("native all-directories view restores three independent file groups");
    const named = name => snapshot.roots.find(root => root.root.path.endsWith(`/${name}`)).root;
    const queryRoot = (root, mode = "uncommitted") => invoke("core_review_query", { query: { taskId, rootId: root.id, mode } });
    const selectRootFile = async name => {
      const root = named(name);
      const file = await waitFor(() => panel()?.querySelector(`[data-root-id="${root.id}"] button[aria-label="shared.txt"]:not(:disabled)`), "mutation root selection");
      file.click();
      await waitFor(() => panel()?.querySelector(".review-file-body code"), "mutation body loaded");
    };
    const openFileMenu = async path => {
      const file = await waitFor(() => panel()?.querySelector(`.review-root[data-active="true"] button[aria-label="${path}"]`), "context file");
      file.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, clientX: 200, clientY: 250 }));
      await waitFor(() => panel()?.querySelector('[role="menu"]'), "file context menu");
    };
    const a = named("root-a"), b = named("root-b"), notes = named("root-notes");
    await selectRootFile("root-a");
    await openFileMenu("shared.txt");
    button("暂存更改").click();
    await waitFor(async () => (await queryRoot(a, "staged")).roots[0].files.length === 1, "stage completion");
    assert((await queryRoot(b, "staged")).roots[0].files.length === 0, "stage crossed repository boundary");
    checks.push("native staging in root-a leaves root-b index untouched");
    const mode = panel().querySelector('select[aria-label="比较模式"]');
    mode.value = "staged"; mode.dispatchEvent(new Event("change", { bubbles: true }));
    await new Promise(resolve => setTimeout(resolve, 80));
    (await waitFor(() => panel()?.querySelector('button[aria-label="shared.txt"]:not(:disabled)'), "staged file")).click();
    await openFileMenu("shared.txt");
    button("取消暂存更改").click();
    await waitFor(async () => (await queryRoot(a, "staged")).roots[0].files.length === 0, "unstage completion");
    assert((await queryRoot(b, "staged")).roots[0].files.length === 0, "unstage crossed repository boundary");
    checks.push("native unstaging restores only root-a index");
    await selectRootFile("root-notes");
    await openFileMenu("shared.txt");
    button("丢弃更改…").click();
    const modal = () => panel()?.querySelector('dialog[aria-label="丢弃文件更改"]');
    await waitFor(() => modal()?.matches(":modal"), "non-Git discard modal");
    assert(modal().textContent.includes(`${notes.path}/shared.txt`), "discard scope is wrong");
    [...modal().querySelectorAll("button")].find(node => node.textContent === "丢弃更改").click();
    await waitFor(async () => !modal() && (await queryRoot(notes)).roots[0].files.length === 0, "non-Git discard complete");
    for (const root of [a, b]) assert((await queryRoot(root)).roots[0].files.length === 1, "discard modified sibling repository");
    checks.push("native non-Git discard affects only selected project directory");
    await selectRootFile("root-a");
    const historyMode = panel().querySelector('select[aria-label="比较模式"]');
    historyMode.value = "committed"; historyMode.dispatchEvent(new Event("change", { bubbles: true }));
    const commits = await waitFor(() => {
      const select = panel()?.querySelector('select[aria-label="比较提交"]');
      return [...(select?.options ?? [])].some(option => option.textContent.includes("fixture merge")) && select;
    }, "merge commit option");
    commits.value = [...commits.options].find(option => option.textContent.includes("fixture merge")).value;
    commits.dispatchEvent(new Event("change", { bubbles: true }));
    const parents = await waitFor(() => panel()?.querySelector('select[aria-label="父提交"]'), "merge parent selector");
    assert(parents.options.length === 2, "merge must expose both parents");
    for (const [parent, path, text] of [[0, "side-only.txt", "side branch contribution"], [1, "main-only.txt", "main branch contribution"]]) {
      parents.value = String(parent); parents.dispatchEvent(new Event("change", { bubbles: true }));
      const file = await waitFor(() => panel()?.querySelector(`button[aria-label="${path}"]:not(:disabled)`), "parent-specific file");
      file.click();
      await waitFor(() => panel()?.querySelector(".review-file-body")?.textContent.includes(text), "parent-specific body");
      assert(panel().querySelectorAll('nav[aria-label="Review 文件列表"] .review-file-entry').length === 1, "merge parent file set incorrect");
      await openFileMenu(path);
      assert(!button("暂存更改") && !button("丢弃更改…"), "history mode exposes working mutation");
      button("打开更改").click();
      checks.push(`native merge parent ${parent + 1} shows its own file and read-only body`);
    }
  } catch (cause) { error = String(cause); }
  await invoke("report", { checks, error });
})();
