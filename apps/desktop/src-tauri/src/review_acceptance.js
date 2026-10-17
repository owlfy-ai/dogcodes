// Embedded only by the isolated review-acceptance build. All disk operations
// target the task seeded in its fresh temporary database, never caller paths.
(async () => {
  const checks = [];
  const invoke = (...args) => window.__TAURI_INTERNALS__.invoke(args[0], ["core_review_git_action", "core_review_index", "core_review_discard", "core_review_ignore", "core_review_batch_files"].includes(args[0]) ? { operationId: crypto.randomUUID(), ...args[1] } : args[1]).catch(cause => {
    throw new Error(`${args[0]}: ${String(cause)}`);
  });
  const assert = (condition, message) => { if (!condition) throw new Error(message); };
  let error = null;
  let leaseTimer;
  let releaseLease;
  // Geometry reads force native layout. Bound the settle delay so an occluded
  // WebView cannot suspend this harness indefinitely at requestAnimationFrame.
  const settle = () => new Promise(resolve => setTimeout(resolve, 80));
  try {
    const bootstrap = await invoke("core_bootstrap");
    const taskId = bootstrap.tasks[0].taskId;
    const query = () => invoke("core_review_query", { query: { taskId, mode: "uncommitted" } });
    const snapshot = await query();
    const leaseId = crypto.randomUUID();
    let leaseRevision = 0;
    const lease = release => invoke("core_review_snapshot_lease", { request: { taskId, snapshotId: snapshot.id, leaseId, revision: ++leaseRevision, release } });
    releaseLease = () => lease(true);
    await lease(false);
    leaseTimer = setInterval(() => {
      void lease(false)
        .catch(cause => { error ??= `acceptance snapshot lease: ${String(cause)}`; });
    }, 30000);
    const root = snapshot.roots.find(root => root.files.some(file => file.path === "review-example.txt"));
    assert(root && !root.error, "seeded comparison missing or incomplete");
    const fileArgs = { taskId, snapshotId: snapshot.id, rootId: root.root.id, path: "review-example.txt" };
    const original = await invoke("core_review_file", fileArgs);
    assert(original.text.before.join("").includes("before"), "baseline text missing");
    assert(original.text.after.join("").includes("added"), "working text missing");
    checks.push("native query and frozen body");
    // Exercise the actual App/React controls and native backend, not a fixture
    // page or mocked API. DOM events do not claim physical pointer coverage.
    const waitFor = async (read, label) => {
      const deadline = Date.now() + 20000;
      while (Date.now() < deadline) {
        const result = read();
        if (result) return result;
        await new Promise(resolve => setTimeout(resolve, 50));
      }
      throw new Error(`UI timed out: ${label}; alerts: ${[...document.querySelectorAll('[role="alert"]')].map(element => element.textContent).join(" | ")}`);
    };
    const panel = () => document.querySelector('[aria-label="工作区 Review"]');
    const button = (label) => [...document.querySelectorAll("button")].find(node => node.textContent === label && !node.disabled);
    (await waitFor(() => button("Diff"), "App Diff button")).click();
    await waitFor(() => panel()?.querySelector('button[aria-label="review-example.txt"]'), "non-Git file tree");
    panel().querySelector('button[aria-label="review-example.txt"]').click();
    await waitFor(() => panel()?.querySelector("code") && panel().textContent.includes("added"), "non-Git body");
    checks.push("native UI opens Review and reads non-Git file");
    const dialog = () => panel()?.querySelector('dialog[aria-label="丢弃文件更改"]');
    const dialogButton = label => [...(dialog()?.querySelectorAll("button") ?? [])].find(node => node.textContent === label && !node.disabled);
    const openFileMenu = async path => {
      const entry = await waitFor(() => panel()?.querySelector(`button[aria-label="${path}"]`), "file menu target");
      entry.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, clientX: 200, clientY: 250 }));
      return waitFor(() => panel()?.querySelector('[role="menu"]'), "file context menu");
    };
    await openFileMenu("review-example.txt");
    button("丢弃更改…").click();
    await waitFor(() => dialog()?.matches(":modal"), "native modal top layer");
    assert(dialog().textContent.includes("review-example.txt"), "confirmation omits full affected path");
    assert(document.activeElement === dialogButton("取消"), "safe cancel does not receive initial focus");
    dialogButton("取消").click();
    await waitFor(() => !dialog(), "cancel closes confirmation");
    assert(document.activeElement?.getAttribute("aria-label") === "review-example.txt", "cancel does not restore file focus");
    assert(JSON.stringify(await invoke("core_review_file", { ...fileArgs, snapshotId: (await query()).id })) === JSON.stringify(original), "cancel changed working contents");
    checks.push("native UI discard confirmation path, modal, cancel and focus");
    await openFileMenu("review-example.txt");
    button("丢弃更改…").click();
    await waitFor(() => dialog()?.matches(":modal"), "discard confirmation reopened");
    dialogButton("丢弃更改").click();
    await waitFor(() => !dialog() && !panel()?.querySelector('[aria-busy="true"]') && !panel()?.querySelector('button[aria-label="review-example.txt"]'), "confirmed discard refreshes tree");
    const uiDiscarded = await query();
    assert(uiDiscarded.roots.filter(entry => !entry.root.git).every(entry => !entry.error && entry.files.length === 0), "UI discard did not restore disk baseline");
    checks.push("native UI confirmed discard restores contents");
    const choose = async (label, value) => {
      const control = await waitFor(() => panel()?.querySelector(`select[aria-label="${label}"]`), label);
      await waitFor(() => [...control.options].some(option => option.value === value), `${label} option ${value}`);
      control.value = value;
      control.dispatchEvent(new Event("change", { bubbles: true }));
      await new Promise(resolve => setTimeout(resolve, 50));
    };
    const uiGit = snapshot.roots.find(entry => entry.root.git);
    assert(uiGit, "UI Git fixture absent");
    assert(uiGit.statistics["renamed.txt"].additions === 1 && uiGit.statistics["renamed.txt"].deletions === 0, "CRLF Git statistics count unchanged lines");
    panel().querySelector('button[aria-label="View as Tree"]').click();
    const rootButton = await waitFor(() => panel()?.querySelector(`[data-root-id="${uiGit.root.id}"] .review-root-heading`), "Git directory");
    rootButton.click();
    await settle();
    if (rootButton.getAttribute("aria-expanded") === "false") rootButton.click();
    await settle();
    for (const [mode, before, after] of [["uncommitted", "committed", "working"], ["unstaged", "staged", "working"], ["staged", "committed", "staged"], ["committed", "base", "committed"], ["branch", "base", "committed"]]) {
      await choose("比较模式", mode);
      if (mode === "committed") {
        const commit = await waitFor(() => panel()?.querySelector('select[aria-label="比较提交"] option:nth-child(2)')?.value, "commit history loaded");
        await choose("比较提交", commit);
      }
      if (mode === "branch") await choose("目标分支", "refs/heads/comparison-base");
      const entry = await waitFor(() => !panel()?.querySelector('[aria-busy="true"]') && panel()?.querySelector('button[aria-label="tracked.txt"]'), `${mode} tree`);
      entry.click();
      await waitFor(() => {
        const lines = [...panel().querySelectorAll("code")].map(node => node.textContent.trim());
        return lines.includes(before) && lines.includes(after);
      }, `${mode} rendered states`);
      checks.push(`native UI ${mode} renders expected states`);
    }
    await choose("比较模式", "unstaged");
    (await waitFor(() => !panel()?.querySelector('[aria-busy="true"]') && panel()?.querySelector('button[aria-label="renamed.txt"]'), "renamed file tree")).click();
    (await waitFor(() => panel()?.querySelector(".review-context button"), "expand long file context")).click();
    await waitFor(() => panel()?.querySelectorAll("code").length === 201, "expanded 200-line context");
    const reader = panel().querySelector(".review-text");
    const scroller = panel().querySelector(".review-file-body");
    assert(scroller.scrollHeight > scroller.clientHeight + 200, "fixture has no actual scroll range");
    scroller.scrollTop = 200;
    await waitFor(() => scroller.scrollTop >= 199, "native scroll position");
    const position = scroller.scrollTop;
    const watchRefresh = () => new Promise((resolve, reject) => {
      const body = panel().querySelector(".review-body");
      let started = false;
      const timer = setTimeout(() => { observer.disconnect(); reject(new Error("UI refresh never completed")); }, 20000);
      const observer = new MutationObserver(records => {
        started ||= body.getAttribute("aria-busy") === "true" || records.some(record => record.oldValue === "true");
        if (started && body.getAttribute("aria-busy") === "false") {
          clearTimeout(timer); observer.disconnect(); resolve();
        }
      });
      observer.observe(body, { attributes: true, attributeFilter: ["aria-busy"], attributeOldValue: true });
    });
    const refreshed = watchRefresh();
    button("刷新").click();
    await refreshed;
    await settle();
    assert(panel().querySelector(".review-text") === reader, "refresh remounted the reader");
    assert(panel().querySelectorAll("code").length === 201, "refresh collapsed expanded context");
    assert(Math.abs(scroller.scrollTop - position) <= 1, "refresh changed actual scroll position");
    assert(panel().querySelector('button[aria-label="renamed.txt"]')?.getAttribute("aria-current") === "true", "refresh lost file selection");
    checks.push("native UI refresh preserves selected file, expanded context and actual scroll position");
    for (const restore of [false, true]) {
      await invoke("mutate_review_fixture", { restore });
      assert(panel().textContent.includes("inserted during review") === restore, "disk change replaced frozen reader before refresh");
      const updated = watchRefresh();
      button("刷新").click();
      await updated;
      await settle();
      assert(panel().querySelector(".review-text") === reader, "content refresh remounted reader");
      assert(panel().querySelectorAll("code").length === (restore ? 201 : 202), "content refresh collapsed split context");
      assert(panel().textContent.includes("inserted during review") !== restore, "content refresh failed to show current disk text");
      assert(Math.abs(scroller.scrollTop - position) <= 1, "content refresh changed scroll position above the edit");
    }
    checks.push("native UI frozen reader and scroll survive real disk insertion and restoration");
    const anchorRow = () => reader.querySelector('[data-review-old-line="170"]');
    assert(anchorRow(), "original-line reading anchor missing");
    scroller.scrollTop += anchorRow().getBoundingClientRect().top - scroller.getBoundingClientRect().top - 24;
    await settle();
    const anchorOffset = anchorRow().getBoundingClientRect().top - scroller.getBoundingClientRect().top;
    assert(anchorOffset >= 0 && anchorOffset < scroller.clientHeight, "reading anchor not in viewport");
    const anchorPosition = scroller.scrollTop;
    for (const restore of [false, true]) {
      await invoke("mutate_review_fixture", { restore });
      const updated = watchRefresh();
      button("刷新").click();
      await updated;
      await settle();
      const offset = anchorRow().getBoundingClientRect().top - scroller.getBoundingClientRect().top;
      assert(Math.abs(offset - anchorOffset) <= 1, "refresh moved the visible original line after an edit above it");
      if (!restore) assert(scroller.scrollTop > anchorPosition + 1, "anchor did not compensate for inserted row height");
      else assert(Math.abs(scroller.scrollTop - anchorPosition) <= 1, "restoration did not recover original scroll position");
    }
    checks.push("native UI original-line anchor survives insertion and deletion above viewport");
    (await waitFor(() => panel()?.querySelector('button[aria-label="untracked.txt"]'), "added file")).click();
    await waitFor(() => panel()?.querySelectorAll('[data-review-new-line]').length === 200, "pure-added rows");
    const splitToggle = [...panel().querySelectorAll('label')].find(node => node.textContent.includes('左右对照')).querySelector('input');
    for (const split of [false, true]) {
      if (splitToggle.checked !== split) splitToggle.click();
      await settle();
      const addedRow = () => [...panel().querySelectorAll('[data-review-new-line]')].find(node => node.querySelector('code')?.textContent === 'added reading line 100');
      assert(addedRow(), 'working reading anchor missing');
      scroller.scrollTop += addedRow().getBoundingClientRect().top - scroller.getBoundingClientRect().top - 24;
      await settle();
      const offsetBefore = addedRow().getBoundingClientRect().top - scroller.getBoundingClientRect().top;
      assert(offsetBefore >= 0 && offsetBefore < scroller.clientHeight, 'added anchor outside viewport');
      for (const restore of [false, true]) {
        await invoke('mutate_review_fixture', { restore, added: true });
        const updated = watchRefresh(); button('刷新').click(); await updated; await settle();
        assert(panel().querySelectorAll('[data-review-new-line]').length === (restore ? 200 : 201), 'added rows not refreshed');
        assert(Math.abs(addedRow().getBoundingClientRect().top - scroller.getBoundingClientRect().top - offsetBefore) <= 1, 'pure-added reading position moved');
      }
      checks.push(`native pure-added anchor survives real insertion and removal (split=${split})`);
    }
    if (splitToggle.checked) splitToggle.click();
    panel().querySelector('button[aria-label="renamed.txt"]').click();
    await waitFor(() => panel()?.querySelector('.review-file-heading')?.textContent.includes('renamed.txt') && panel()?.textContent.includes('edited after moving'), 'return to rename');
    await openFileMenu("renamed.txt");
    button("丢弃更改…").click();
    await waitFor(() => dialog()?.matches(":modal"), "group discard modal");
    assert(dialog().textContent.includes("old.txt") && dialog().textContent.includes("renamed.txt"), "rename confirmation omits one affected path");
    assert(dialog().textContent.includes("同时影响以上两个路径"), "group scope warning missing");
    dialogButton("丢弃更改").click();
    await waitFor(() => !dialog() && !panel()?.querySelector('[aria-busy="true"]') && !panel()?.querySelector('button[aria-label="renamed.txt"]'), "group discard refreshed");
    const afterUiGroup = await query();
    const groupRoot = afterUiGroup.roots.find(entry => entry.root.id === uiGit.root.id);
    assert(groupRoot && !groupRoot.error && !groupRoot.files.some(entry => ["old.txt", "renamed.txt"].includes(entry.path)), "group discard left path differences");
    assert(groupRoot.files.some(entry => entry.path === "tracked.txt"), "group discard lost unrelated changes");
    checks.push("native UI rename two-path confirmation and grouped discard");
    panel().querySelector('button[aria-label="关闭 Review"]').click();
    await waitFor(() => !panel(), "Review closes");
    const request = { taskId, snapshotId: snapshot.id, rootId: root.root.id, path: fileArgs.path, confirmed: false };
    let rejected = false;
    try { await invoke("core_review_discard", { request }); } catch { rejected = true; }
    assert(rejected, "unconfirmed discard accepted");
    checks.push("unconfirmed discard rejected");
    const frozen = await invoke("core_review_file", fileArgs);
    assert(JSON.stringify(frozen) === JSON.stringify(original), "old snapshot changed after discard");
    checks.push("snapshot body remains frozen after disk mutation");
    const gitRoot = snapshot.roots.find(root => root.root.git);
    assert(gitRoot && !gitRoot.error, "Git fixture missing");
    const rootId = gitRoot.root.id;
    const gitQuery = async (mode, reference) => {
      const result = await invoke("core_review_query", { query: { taskId, rootId, mode, reference } });
      // The script consumes each snapshot synchronously; release the reader
      // lease so a long acceptance run exercises normal cache eviction.
      await invoke("core_review_snapshot_lease", { request: { taskId, snapshotId: result.id, leaseId: crypto.randomUUID(), revision: 1, release: true } });
      return result;
    };
    const body = async (mode, before, after, reference) => {
      const result = await gitQuery(mode, reference);
      assert(!result.roots[0].error, `${mode}: ${result.roots[0].error}`);
      const file = await invoke("core_review_file", { taskId, snapshotId: result.id, rootId, path: "tracked.txt" });
      assert(file.text.before.join("").trim() === before && file.text.after.join("").trim() === after, `${mode} compared wrong states`);
      checks.push(`Git ${mode} compares expected states`);
      return result;
    };
    await body("uncommitted", "committed", "working");
    await body("unstaged", "staged", "working");
    await body("staged", "committed", "staged");
    const history = await invoke("core_review_history", { taskId, rootId, skip: 0 });
    assert(history.length === 2, "commit history missing");
    await body("committed", "base", "committed", history[0].id);
    const branches = await invoke("core_review_branches", { taskId, rootId });
    assert(branches.some(branch => branch.name.includes("comparison-base")), "branch list missing target");
    await body("branch", "base", "committed", "comparison-base");
    const staging = await gitQuery("unstaged");
    assert(staging.roots[0].untracked.includes("untracked.txt"), "untracked file not separately reported");
    await invoke("core_review_index", { taskId, rootId, snapshotId: staging.id, paths: ["tracked.txt"], action: "stage" });
    await body("staged", "committed", "working");
    const staged = await gitQuery("staged");
    await invoke("core_review_index", { taskId, rootId, snapshotId: staged.id, paths: ["tracked.txt"], action: "unstage" });
    const unstaged = await gitQuery("staged");
    assert(!unstaged.roots[0].error && unstaged.roots[0].files.length === 0, "unstage left index changes");
    await body("unstaged", "committed", "working");
    checks.push("Git stage and unstage preserve working contents");
    const beforeDiscard = await gitQuery("unstaged");
    await invoke("core_review_discard", { request: { taskId, rootId, snapshotId: beforeDiscard.id, path: "tracked.txt", confirmed: true } });
    const afterDiscard = await gitQuery("unstaged");
    assert(!afterDiscard.roots[0].error && !afterDiscard.roots[0].files.some(file => file.path === "tracked.txt"), `Git discard refresh: ${JSON.stringify(afterDiscard.roots[0])}`);
    checks.push("Git CRLF discard through native IPC");
  } catch (cause) {
    error = String(cause);
  }
  clearInterval(leaseTimer);
  if (releaseLease) await releaseLease().catch(() => {});
  await invoke("report", { checks, error });
})();
