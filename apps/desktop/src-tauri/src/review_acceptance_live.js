// Explicit opt-in: two real model edits in the seeded temporary project only.
(async () => {
  const invoke = (...args) => window.__TAURI_INTERNALS__.invoke(...args);
  const checks = [];
  const assert = (value, message) => { if (!value) throw new Error(message); };
  const button = label => [...document.querySelectorAll("button")].find(node => node.getAttribute("aria-label") === label && !node.disabled);
  const waitFor = async (read, label, milliseconds = 20000) => {
    const deadline = Date.now() + milliseconds;
    while (Date.now() < deadline) { const value = await read(); if (value) return value; await new Promise(resolve => setTimeout(resolve, 100)); }
    throw new Error(label);
  };
  let error = null;
  try {
    const task = (await invoke("core_bootstrap")).tasks.find(task => task.title === "真实模型 Diff 验收");
    const providerId = task?.preferredExecutor.providerId;
    assert(["codex", "claude-code"].includes(providerId), "real agent task missing");
    const project = (await invoke("core_bootstrap")).projects.find(project => project.projectId === task.projectId);
    assert(project?.path, "isolated project missing");
    const taskId = task.taskId;
    let previous = "before real model edit";
    (await waitFor(() => document.querySelector('button.task[data-tooltip="真实模型 Diff 验收"]'), "real task row missing")).click();
    for (const value of ["first real model edit", "second real model edit"]) {
      const approved = new Set();
      const before = await invoke("core_bootstrap");
      const input = await waitFor(() => document.querySelector("form.composer textarea"), "composer missing");
      const prompt = `Isolated integration test. Use ${providerId === "claude-code" ? "Write (you may Read sample.txt first)" : "apply_patch"} to replace the entire content of sample.txt in the current working directory with exactly one line: ${value}\n. Modify only sample.txt. Do not use shell commands, read other paths, or make other changes. Then reply DONE.`;
      Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value").set.call(input, prompt);
      input.dispatchEvent(new Event("input", { bubbles: true }));
      (await waitFor(() => button("发送"), "send control missing")).click();
      await waitFor(async () => {
        const events = await invoke("core_sync_events", { afterSequence: before.lastSequence, limit: 10000 });
        const pending = await invoke("core_pending_approvals", { taskId, limit: 20 });
        for (const approval of pending) {
          if (approved.has(approval.approvalId)) continue;
          assert(approved.size < 3, "repeated approval");
          const summary = JSON.parse(approval.commandSummary);
          if (providerId === "claude-code") {
            assert(["Read", "Write"].includes(approval.toolName), "unexpected Claude tool");
            assert(summary.file_path === `${project.path}/sample.txt` || summary.file_path === "sample.txt", "Claude path outside fixture");
            if (approval.toolName === "Write") assert(summary.content === `${value}\n`, "Claude content differs from exact fixture edit");
          } else {
          assert(approval.toolName === "fileChange", "unexpected Codex tool");
          assert(summary.scope === "reported_files" && summary.detailsSource === "matching_item_started" && !summary.grantRoot, "unverified approval source or root grant");
          assert(summary.files?.length === 1, "approval must affect exactly one file");
          const file = summary.files[0];
          assert(file.path?.relativeTo === "current_working_directory" && file.path.path === "sample.txt" && file.kind.movePath === null, "approval targets unexpected path");
          const exactDelete = file.kind.type === "delete" && file.diff === `${previous}\n`;
          const exactAdd = file.kind.type === "add" && file.diff === `${value}\n`;
          const exactUpdate = file.kind.type === "update" && file.diff === `@@ -1 +1 @@\n-${previous}\n+${value}\n`;
          assert(exactDelete || exactAdd || exactUpdate, "patch is not the exact isolated fixture edit; no approval granted");
          }
          const card = await waitFor(() => [...document.querySelectorAll(".approval-card")].find(node => node.dataset.approvalId === approval.approvalId), "approval card missing");
          assert(card.textContent.includes("sample.txt"), "approval path not visible");
          const allow = await waitFor(() => [...card.querySelectorAll("button")].find(node => node.textContent === "批准一次" && !node.disabled), "allow-once control missing");
          approved.add(approval.approvalId);
          allow.click();
        }
        if (events.some(event => event.eventType === "turn.failed")) throw new Error("real model turn failed");
        return events.some(event => event.eventType === "turn.completed");
      }, "real model turn timed out", 120000);
      const snapshot = await invoke("core_review_query", { query: { taskId, mode: "uncommitted" } });
      assert(snapshot.roots.length === 1 && !snapshot.roots[0].root.git, "live project must remain non-Git");
      const root = snapshot.roots[0];
      assert(!root.error && root.files.length === 1 && root.files[0].path === "sample.txt", "real edit affected wrong files");
      const file = await invoke("core_review_file", { taskId, snapshotId: snapshot.id, rootId: root.root.id, path: "sample.txt" });
      assert(file.text.before.join("") === "before real model edit\n", "original baseline changed between turns");
      assert(file.text.after.join("") === `${value}\n`, "model did not make requested exact edit");
      checks.push(`desktop composer real ${providerId} edit: ${value}; original baseline preserved`);
      previous = value;
    }
    const diff = await waitFor(() => [...document.querySelectorAll("button")].find(node => node.textContent === "Diff" && !node.disabled), "Diff control missing");
    diff.click();
    (await waitFor(() => document.querySelector('button[aria-label="sample.txt"]:not(:disabled)'), "edited file missing from Review")).click();
    await waitFor(() => document.querySelector(".review-file-body")?.textContent.includes("second real model edit"), "final model edit not rendered");
    assert(document.querySelector(".review-file-body").textContent.includes("before real model edit"), "original text not rendered");
    checks.push("native Review renders first baseline against final real model edit");
  } catch (cause) {
    error = String(cause);
    button("停止")?.click();
  }
  await invoke("report", { checks, error });
})();
