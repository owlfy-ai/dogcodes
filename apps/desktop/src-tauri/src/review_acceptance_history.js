// Read-only follow-up of the real mixed-agent fixture after committing its index.
(async () => {
  const invoke = (...args) => window.__TAURI_INTERNALS__.invoke(...args);
  const assert = (value, message) => { if (!value) throw new Error(message); };
  const checks = [];
  let error = null;
  try {
    const bootstrap = await invoke('core_bootstrap');
    const taskFor = suffix => {
      const project = bootstrap.projects.find(project => project.path.endsWith(`/${suffix}`));
      return bootstrap.tasks.find(task => task.projectId === project?.projectId);
    };
    const task = taskFor('live-git-project');
    assert(task, 'mixed Git task missing');
    const taskId = task.taskId;
    const initial = await invoke('core_review_query', { query: { taskId, mode: 'uncommitted' } });
    assert(initial.roots.length === 1 && initial.roots[0].root.git, 'single Git root required');
    const rootId = initial.roots[0].root.id;
    const history = await invoke('core_review_history', { taskId, rootId, skip: 0 });
    assert(history.length === 2 && history[0].subject === 'mixed agent staged edit', 'expected real staged edit commit');
    const verify = async (mode, before, after, reference) => {
      const snapshot = await invoke('core_review_query', { query: { taskId, rootId, mode, reference } });
      const root = snapshot.roots[0];
      assert(!root.error && root.files.length === 1 && root.files[0].path === 'sample.txt', `${mode}: incorrect file set`);
      const body = await invoke('core_review_file', { taskId, snapshotId: snapshot.id, rootId, path: 'sample.txt' });
      assert(body.text.before.join('') === before && body.text.after.join('') === after, `${mode}: wrong content`);
      checks.push(`${mode}: exact real-model content after process restart`);
    };
    await verify('committed', 'before real model edit\n', 'mixed edit 1\n', history[0].id);
    await verify('committed', '', 'before real model edit\n', history[1].id);
    await verify('branch', 'before real model edit\n', 'mixed edit 1\n', 'review-acceptance-base');
    await verify('uncommitted', 'mixed edit 1\n', 'mixed edit 3\n');
    await verify('unstaged', 'mixed edit 1\n', 'mixed edit 3\n');
    const staged = await invoke('core_review_query', { query: { taskId, rootId, mode: 'staged' } });
    assert(!staged.roots[0].error && staged.roots[0].files.length === 0, 'committed index should be clean');
    checks.push('staged: empty after committing first model edit');
    const nonGit = taskFor('live-project');
    assert(nonGit, 'mixed non-Git task missing');
    const snapshot = await invoke('core_review_query', { query: { taskId: nonGit.taskId, mode: 'uncommitted' } });
    const root = snapshot.roots[0];
    assert(snapshot.roots.length === 1 && !root.root.git && !root.error && root.files.length === 1, 'non-Git state lost on restart');
    const body = await invoke('core_review_file', { taskId: nonGit.taskId, snapshotId: snapshot.id, rootId: root.root.id, path: 'sample.txt' });
    assert(body.text.before.join('') === 'before real model edit\n' && body.text.after.join('') === 'mixed edit 3\n', 'non-Git baseline changed on restart');
    checks.push('non-Git: original baseline survives full desktop process restart after agent switches');
  } catch (cause) { error = String(cause); }
  await invoke('report', { checks, error });
})();
