// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, expect, it, vi } from "vitest";
import type { AgentUpdate, CoreClient, InstallationSummary } from "@dogcodes/protocol";
import { AgentUpdateControl, useAgentUpdates } from "./agentUpdates";
const i: InstallationSummary = { installationId: "ins_a", providerId: "kimi", executable: "/kimi", fingerprint: "f", version: "0.42.0", versionText: null, status: "ready", diagnostic: null, probedAt: "2026-09-14", probeElapsedMillis: 1 };
const available: AgentUpdate = { installationId: i.installationId, status: "available", currentVersion: i.version, latestVersion: "0.43.0", source: "native", message: null, canUpgrade: true, checkedAt: Date.now() };
let root: Root; let element: HTMLDivElement;
afterEach(async () => { if (root) await act(async () => root.unmount()); element?.remove(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });
async function mount(node: React.ReactNode) { vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true); element=document.createElement("div"); document.body.append(element); root=createRoot(element); await act(async () => root.render(node)); }
it("checks on mount and upgrades only once after a click, retaining status across card navigation", async () => {
  let finish!: (u: AgentUpdate) => void;
  const upgradeAgent=vi.fn(() => new Promise<AgentUpdate>(resolve => { finish=resolve; }));
  const checkAgentUpdate=vi.fn(async () => available);
  const refresh=vi.fn();
  const client={ checkAgentUpdate, upgradeAgent, bootstrap: async () => ({ installations: [i] }) } as unknown as CoreClient;
  function Harness({ show=true }: {show?: boolean}) { const actions=useAgentUpdates(client,[i],refresh);return show ? <AgentUpdateControl id={i.installationId} actions={actions} /> : null; }
  await mount(<Harness />); expect(checkAgentUpdate).toHaveBeenCalledWith(i.installationId,false); expect(upgradeAgent).not.toHaveBeenCalled();
  const button=element.querySelector("button")!;
  await act(async () => { button.click(); button.click(); });
  expect(upgradeAgent).toHaveBeenCalledTimes(1);expect(element.querySelector('[role="status"]')?.getAttribute("aria-label")).toContain("正在升级");expect(element.querySelector("button")).toBeNull();
  await act(async () => root.render(<Harness show={false} />));
  await act(async () => finish({...available,status:"updated",canUpgrade:false}));
  await act(async () => root.render(<Harness />));
  expect(element.querySelector('[role="status"]')?.getAttribute("aria-label")).toContain("升级完成");expect(refresh).toHaveBeenCalledTimes(1);
});
it("keeps errors actionable and never renders success for a failed check", async () => {
  const check=vi.fn();await mount(<AgentUpdateControl id={i.installationId} actions={{updates:{[i.installationId]:{...available,status:"error",message:"网络不可用",canUpgrade:false}},check,upgrade:vi.fn()}}/>);
  expect(element.querySelector("button")?.dataset.tooltip).toContain("网络不可用");expect(element.textContent).not.toContain("已是最新");await act(async () => element.querySelector("button")!.click());expect(check).toHaveBeenCalledWith(i.installationId,true);
});
it("retries the upgrade after a failed upgrade instead of merely checking the feed", async () => {
  const upgradeAgent = vi.fn().mockRejectedValueOnce("下载超时").mockResolvedValueOnce({ ...available, status: "updated", canUpgrade: false });
  const checkAgentUpdate = vi.fn(async () => available);
  const client = { checkAgentUpdate, upgradeAgent, bootstrap: async () => ({ installations: [i] }) } as unknown as CoreClient;
  function Harness() { const actions = useAgentUpdates(client, [i], vi.fn()); return <AgentUpdateControl id={i.installationId} actions={actions} />; }
  await mount(<Harness />);
  await act(async () => element.querySelector("button")!.click());
  expect(element.querySelector("button")?.getAttribute("aria-label")).toBe("重试升级：下载超时");
  await act(async () => element.querySelector("button")!.click());
  expect(upgradeAgent).toHaveBeenCalledTimes(2);
  expect(checkAgentUpdate).toHaveBeenCalledTimes(1);
  expect(element.textContent).not.toContain("有新版本");
});
it("upgrades different Agents concurrently while preventing duplicate submits for each installation", async () => {
  const other: InstallationSummary = { ...i, installationId: "ins_b", providerId: "claude-code" };
  const finishes = new Map<string, (value: AgentUpdate) => void>();
  const upgradeAgent = vi.fn((id: string) => new Promise<AgentUpdate>(resolve => { finishes.set(id, resolve); }));
  const refresh = vi.fn();
  const client = { checkAgentUpdate: vi.fn(async (id: string) => ({ ...available, installationId: id })), upgradeAgent, bootstrap: async () => ({ installations: [i, other] }) } as unknown as CoreClient;
  let actions!: ReturnType<typeof useAgentUpdates>;
  function Harness() { actions = useAgentUpdates(client, [i, other], refresh); return <><AgentUpdateControl id={i.installationId} actions={actions} /><AgentUpdateControl id={other.installationId} actions={actions} /></>; }
  await mount(<Harness />);
  await act(async () => element.querySelector("button")!.click());
  expect(upgradeAgent).toHaveBeenCalledTimes(1);
  expect(element.textContent).toContain("正在检查升级条件");
  expect(element.querySelector("button")!.disabled).toBe(false);
  expect(element.querySelector("button")!.dataset.tooltip).not.toContain("请等待");
  await act(async () => { element.querySelector("button")!.click(); void actions.upgrade(i.installationId); void actions.upgrade(other.installationId); });
  expect(upgradeAgent.mock.calls).toEqual([[i.installationId, undefined], [other.installationId, undefined]]);
  expect(element.querySelectorAll('[role="status"]')).toHaveLength(2);
  await act(async () => finishes.get(other.installationId)!({ ...available, installationId: other.installationId, status: "updated", canUpgrade: false }));
  expect(actions.updates[i.installationId].status).toBe("upgrading");
  expect(actions.updates[other.installationId].status).toBe("updated");
  await act(async () => { void actions.upgrade(i.installationId); });
  expect(upgradeAgent).toHaveBeenCalledTimes(2);
  await act(async () => finishes.get(i.installationId)!({ ...available, status: "updated", canUpgrade: false }));
  expect(refresh).toHaveBeenCalledTimes(2);
});
it("allows retrying an upgrade while another installation is upgrading", async () => {
  const upgrade = vi.fn();
  await mount(<AgentUpdateControl id={i.installationId} actions={{
    updates: {
      ins_other: { ...available, installationId: "ins_other", status: "upgrading", canUpgrade: false },
      [i.installationId]: { ...available, status: "error", message: "下载超时" },
    }, check: vi.fn(), upgrade,
  }} />);
  expect(element.querySelector("button")!.disabled).toBe(false);
  await act(async () => element.querySelector("button")!.click());
  expect(upgrade).toHaveBeenCalledWith(i.installationId);
});
it("renders backend download and verification stages and retries backend failures", async () => {
  const upgrade = vi.fn(); const check = vi.fn();
  const actions = { updates: { [i.installationId]: { ...available, status: "upgrading" as const, canUpgrade: false, message: "正在下载并安装，较大的更新可能需要几分钟" } }, upgrade, check };
  await mount(<AgentUpdateControl id={i.installationId} actions={actions} />);
  expect(element.textContent).toContain("正在下载并安装");
  expect(element.textContent).toContain("0 秒");
  await act(async () => root.render(<AgentUpdateControl id={i.installationId} actions={{ ...actions, updates: { [i.installationId]: { ...available, status: "error", message: "更新命令未成功" } } }} />));
  await act(async () => element.querySelector("button")!.click());
  expect(upgrade).toHaveBeenCalledWith(i.installationId);
  expect(check).not.toHaveBeenCalled();
});

it("rescans the opened host on return before forcing a new update check", async () => {
  const host = { ...i, providerId: "zcode", version: "0.16.5" };
  const snapshot = { installations: [{ ...host, version: "0.16.9" }] };
  const checkAgentUpdate = vi.fn(async () => ({ ...available, status: "managed" as const, currentVersion: "3.14.2", latestVersion: "3.14.3" }));
  let finish!: (value: typeof snapshot) => void;
  const scanAgent = vi.fn(() => new Promise<typeof snapshot>(resolve => { finish = resolve; }));
  const openAgentUpdater = vi.fn(async () => {});
  const refresh = vi.fn();
  const client = { checkAgentUpdate, scanAgent, openAgentUpdater } as unknown as CoreClient;
  function Harness() { const actions = useAgentUpdates(client, [host], refresh); return <AgentUpdateControl id={host.installationId} actions={actions} />; }
  await mount(<Harness />);
  await act(async () => { window.dispatchEvent(new Event("focus")); });
  expect(scanAgent).not.toHaveBeenCalled();
  await act(async () => element.querySelector("button")!.click());
  expect(openAgentUpdater).toHaveBeenCalledWith(host.installationId);
  await act(async () => { window.dispatchEvent(new Event("focus")); window.dispatchEvent(new Event("focus")); });
  expect(scanAgent).toHaveBeenCalledExactlyOnceWith("zcode");
  expect(checkAgentUpdate).toHaveBeenCalledTimes(1);
  await act(async () => finish(snapshot));
  expect(refresh).toHaveBeenCalledWith(snapshot);
  expect(checkAgentUpdate).toHaveBeenLastCalledWith(host.installationId, true);
});
it("only shows the host update action when a newer host version is confirmed", async () => {
  const openUpdater=vi.fn();const upgrade=vi.fn();
  const actions={updates:{[i.installationId]:{...available,status:"managed" as const,message:"随桌面应用更新",canUpgrade:false,latestVersion:null as string|null}},check:vi.fn(),upgrade,openUpdater};
  await mount(<AgentUpdateControl id={i.installationId} actions={actions}/>);
  expect(element.querySelector(".agent-update")).toBeNull();
  for (const latestVersion of ["0.41.0", "0.42.0"]) {
    actions.updates[i.installationId].latestVersion=latestVersion;
    await act(async()=>root.render(<AgentUpdateControl id={i.installationId} actions={actions}/>));
    expect(element.querySelector(".agent-update")).toBeNull();
  }
  actions.updates[i.installationId].latestVersion="0.43.0";
  await act(async()=>root.render(<AgentUpdateControl id={i.installationId} actions={actions}/>));
  expect(element.textContent).toContain("桌面应用有新版本 0.43.0");
  await act(async()=>element.querySelector("button")!.click());expect(openUpdater).toHaveBeenCalledWith(i.installationId);expect(upgrade).not.toHaveBeenCalled();
});

it("shows Codex desktop updates using host versions while leaving the bundled CLI alone", async () => {
  const openUpdater = vi.fn(); const upgrade = vi.fn();
  await mount(<AgentUpdateControl id="codex-bundled" actions={{
    updates: { "codex-bundled": { ...available, installationId: "codex-bundled", status: "managed", currentVersion: "26.911.61220", latestVersion: "26.915.31945", source: "Codex 桌面应用 · 官方更新源", canUpgrade: false } },
    check: vi.fn(), upgrade, openUpdater,
  }} />);
  expect(element.textContent).toContain("桌面应用有新版本 26.915.31945");
  expect(element.querySelectorAll("button")).toHaveLength(1);
  await act(async () => element.querySelector("button")!.click());
  expect(openUpdater).toHaveBeenCalledWith("codex-bundled");
  expect(upgrade).not.toHaveBeenCalled();
});

it("requires explicit consent before ending tasks and cancellation leaves them running", async () => {
  HTMLDialogElement.prototype.showModal = function () { this.setAttribute("open", ""); };
  HTMLDialogElement.prototype.close = function () { this.removeAttribute("open"); };
  const inUse: AgentUpdate = { ...available, status: "in_use", canUpgrade: false, inUse: { token: "consent-1", taskCount: 1, processCount: 2 } };
  let finish!: (value: AgentUpdate) => void;
  const upgradeAgent = vi.fn().mockResolvedValueOnce(inUse).mockResolvedValueOnce(inUse)
    .mockImplementationOnce(() => new Promise<AgentUpdate>(resolve => { finish = resolve; }));
  const client = { checkAgentUpdate: vi.fn(async () => available), upgradeAgent, bootstrap: vi.fn(async () => ({ installations: [i] })) } as unknown as CoreClient;
  function Harness() { const actions = useAgentUpdates(client, [i], vi.fn()); return <AgentUpdateControl id={i.installationId} actions={actions} />; }
  await mount(<Harness />);
  await act(async () => element.querySelector("button")!.click());
  let dialog = document.querySelector("dialog")!;
  expect(dialog.textContent).toContain("Agent 正在使用中");
  expect(document.activeElement).toBe(dialog.querySelector("h2"));
  expect(upgradeAgent).toHaveBeenCalledTimes(1);
  await act(async () => (dialog.querySelector("button") as HTMLButtonElement).click());
  expect(document.querySelector("dialog")).toBeNull();
  expect(upgradeAgent).toHaveBeenCalledTimes(1);
  await act(async () => element.querySelector("button")!.click());
  dialog = document.querySelector("dialog")!;
  const confirm = dialog.querySelectorAll("button")[1];
  await act(async () => { confirm.click(); confirm.click(); });
  expect(upgradeAgent).toHaveBeenCalledTimes(3);
  expect(upgradeAgent).toHaveBeenLastCalledWith(i.installationId, "consent-1");
  await act(async () => finish({ ...available, status: "updated", currentVersion: "0.43.0", canUpgrade: false }));
  expect(document.querySelector("dialog")).toBeNull();
  expect(element.querySelector('[role="status"]')?.getAttribute("aria-label")).toBe("升级完成");
});

it("keeps the detailed host updater accessible when its latest version is unknown", async () => {
  const openUpdater = vi.fn();
  await mount(<AgentUpdateControl id={i.installationId} detailed actions={{ updates: { [i.installationId]: { ...available, status: "managed", latestVersion: null, canUpgrade: false, message: "在宿主应用中检查更新" } }, check: vi.fn(), upgrade: vi.fn(), openUpdater }} />);
  expect(element.textContent).toContain("在宿主应用中检查更新");
  await act(async () => element.querySelector("button")!.click());
  expect(openUpdater).toHaveBeenCalledWith(i.installationId);
});
