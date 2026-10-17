// Explicit opt-in. Real agents may edit only sample.txt in two seeded projects.
(async () => {
  const invoke = (...args) => window.__TAURI_INTERNALS__.invoke(...args);
  const checks = [];
  const assert = (value, message) => { if (!value) throw new Error(message); };
  let error = null;
  let activeTask;
  const { TauriCoreClient } = await import('/src/tauriClient.ts');
  const { submitWithAutomaticContext } = await import('/src/automaticContext.ts');
  const client = new TauriCoreClient();
  const read = async (taskId, mode, before, after) => {
    let snapshot = await invoke('core_review_query', { query: { taskId, mode: 'uncommitted' } });
    assert(snapshot.roots.length === 1, 'expected exactly one project root');
    if (mode !== 'uncommitted') {
      snapshot = await invoke('core_review_query', { query: { taskId, mode, rootId: snapshot.roots[0].root.id } });
    }
    const root = snapshot.roots[0];
    assert(!root.error && root.files.length === 1 && root.files[0].path === 'sample.txt', 'unexpected changed file set');
    const file = await invoke('core_review_file', { taskId, snapshotId: snapshot.id, rootId: root.root.id, path: 'sample.txt' });
    assert(file.text.before.join('') === `${before}\n`, `${mode}: wrong baseline`);
    assert(file.text.after.join('') === `${after}\n`, `${mode}: wrong final content`);
    return { snapshot, root };
  };
  try {
    const initial = await client.bootstrap();
    const tasks = ['mixed-git', 'mixed-non-git'].map(title => initial.tasks.find(task => task.title === title));
    assert(tasks.every(Boolean), 'mixed fixture tasks missing');
    const executors = tasks.map(task => task.preferredExecutor);
    assert(executors[0].providerId === 'codex' && executors[1].providerId === 'claude-code', 'real drivers missing');
    for (const [index, task] of tasks.entries()) {
      activeTask = task.taskId;
      const project = initial.projects.find(project => project.projectId === task.projectId);
      assert(project?.path, 'isolated project missing');
      let previous = 'before real model edit';
      // Switch away and back, in opposite directions for Git and non-Git.
      for (const [turn, executor] of [executors[index], executors[1-index], executors[index]].entries()) {
        const value = `mixed edit ${turn + 1}`;
        const before = await client.bootstrap();
        const claude = executor.providerId === 'claude-code';
        await submitWithAutomaticContext(client, task.taskId, executor,
          `Isolated integration test. Use ${claude ? 'Write (you may Read sample.txt first)' : 'apply_patch'} to replace the entire content of sample.txt in the current working directory with exactly one line: ${value}\n. Modify only sample.txt. Do not use shell commands, read other paths, or make other changes. Then reply DONE.`, null);
        const approved = new Set();
        const deadline = Date.now() + 120000;
        let completed = false;
        while (Date.now() < deadline) {
          const pending = await invoke('core_pending_approvals', { taskId: task.taskId, limit: 20 });
          for (const approval of pending) {
            if (approved.has(approval.approvalId)) continue;
            assert(approved.size < 3, 'repeated approvals');
            const summary = JSON.parse(approval.commandSummary);
            if (claude) {
              assert(['Read', 'Write'].includes(approval.toolName), 'unexpected Claude tool');
              assert([`${project.path}/sample.txt`, 'sample.txt'].includes(summary.file_path), 'Claude path outside fixture');
              if (approval.toolName === 'Write') assert(summary.content === `${value}\n`, 'unexpected Claude content');
            } else {
              assert(approval.toolName === 'fileChange' && summary.scope === 'reported_files' && summary.detailsSource === 'matching_item_started' && !summary.grantRoot, 'unverified Codex approval');
              assert(summary.files?.length === 1, 'unexpected Codex file set');
              const file = summary.files[0];
              assert(file.path?.relativeTo === 'current_working_directory' && file.path.path === 'sample.txt' && file.kind.movePath === null, 'Codex path outside fixture');
              assert(file.kind.type === 'delete' && file.diff === `${previous}\n` || file.kind.type === 'add' && file.diff === `${value}\n` || file.kind.type === 'update' && file.diff === `@@ -1 +1 @@\n-${previous}\n+${value}\n`, 'unexpected Codex patch');
            }
            const option = approval.options.find(option => option.semanticRole === 'approve_once');
            assert(option, 'allow-once missing');
            const latest = (await client.bootstrap()).tasks.find(item => item.taskId === task.taskId);
            await client.respondApproval(latest, approval.approvalId, option.optionId, { turnId: approval.turnId, requestedAtSequence: approval.requestedAtSequence });
            approved.add(approval.approvalId);
          }
          const events = await invoke('core_sync_events', { afterSequence: before.lastSequence, limit: 10000 });
          const own = events.filter(event => event.payload?.taskId === task.taskId);
          assert(!own.some(event => event.eventType === 'turn.failed'), 'real model turn failed');
          if (own.some(event => event.eventType === 'turn.completed')) { completed = true; break; }
          await new Promise(resolve => setTimeout(resolve, 100));
        }
        assert(completed, 'real model turn timed out');
        const { snapshot, root } = await read(task.taskId, 'uncommitted', 'before real model edit', value);
        assert(root.root.git === (index === 0), 'wrong project kind');
        if (index === 0 && turn === 0) {
          await invoke('core_review_index', { taskId: task.taskId, snapshotId: snapshot.id, rootId: root.root.id, paths: ['sample.txt'], action: 'stage', operationId: crypto.randomUUID() });
        }
        checks.push(`${task.title}: ${executor.providerId} turn ${turn + 1}; original baseline to actual final file`);
        previous = value;
      }
      if (index === 0) {
        await read(task.taskId, 'staged', 'before real model edit', 'mixed edit 1');
        await read(task.taskId, 'unstaged', 'mixed edit 1', 'mixed edit 3');
        checks.push('mixed Git: HEAD/index/working comparisons remain distinct after agent switches');
      }
    }
  } catch (cause) {
    error = String(cause);
    const task = (await client.bootstrap()).tasks.find(task => task.taskId === activeTask);
    if (task?.activePrimaryRun) await client.cancelTurn(task).catch(() => {});
  }
  await invoke('report', { checks, error });
})();
