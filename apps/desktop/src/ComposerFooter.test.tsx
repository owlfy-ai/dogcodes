// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { expect, it, vi } from "vitest";
import type { CoreClient, TaskSummary } from "@dogcodes/protocol";
import { ComposerFooter } from "./App";
import { PermissionPicker } from "./PermissionPicker";
import { ModelPicker } from "./ModelPicker";

it("replaces both agent controls without leaving duplicate permission menus", async () => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  vi.stubGlobal("ResizeObserver", class { observe() {} disconnect() {} });
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  const errors = vi.spyOn(console, "error");
  const client = {
    permissionCatalog: async () => ({ defaultModeId: "default", modes: [
      { id: "default", label: "默认", description: "请求审批", dangerous: false },
    ] }),
  } as unknown as CoreClient;
  try {
    for (const providerId of ["claude-code", "codex", "kimi", "codex", "claude-code"]) {
      const executor = { providerId, installationId: `ins_${providerId}` };
      const key = `tsk_test:${providerId}:${executor.installationId}`;
      await act(async () => root.render(<ComposerFooter
        selectedTask={{ taskId: "tsk_test", projectId: "prj_test", title: "Test", preferredExecutor: executor, activePrimaryRun: null, streamVersion: 1 } as TaskSummary}
        selectedQuota={null} turnQuotaExhausted={false} executorOptions={[]}
        sending={false} cancelling={false} draft="" onSelect={() => {}} onCancel={() => {}}
        permissionPicker={<PermissionPicker key={key} client={client} executor={executor} taskId="tsk_test" disabled={false} onChange={() => {}} />}
        modelPicker={<ModelPicker key={key} catalog={{ models: [], defaultModel: null }} model={null} thinking={null} loading={false} error={null} disabled={false} onChange={() => {}} onRetry={() => {}} />}
      />));
      expect(container.querySelectorAll('[aria-label="权限策略"]')).toHaveLength(1);
      expect(container.querySelectorAll('[aria-label="选择模型"]')).toHaveLength(1);
      const picker = container.querySelector<HTMLDetailsElement>(".agent-picker")!;
      picker.open = true;
      picker.querySelector("summary")!.dispatchEvent(new Event("pointerdown", { bubbles: true }));
      expect(picker.open).toBe(true);
      document.body.dispatchEvent(new Event("pointerdown", { bubbles: true }));
      expect(picker.open).toBe(false);
      picker.open = true;
      document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
      expect(picker.open).toBe(false);
      expect(document.activeElement).toBe(picker.querySelector("summary"));
    }
    expect(errors.mock.calls.some(args => args.some(value => typeof value === "string" && value.includes("same key")))).toBe(false);
  } finally {
    await act(async () => root.unmount());
    container.remove(); errors.mockRestore(); vi.unstubAllGlobals();
  }
});
