import { RunOperationRejected } from "./projectRunOperations";
import { useEnabledAgents } from "./agentPreferences";
import { ProjectRunItem, RunAnalysisConfirmation } from "./ProjectRunItem";
import { AgentPickerMenu, agentChoiceKey } from "./AgentPickerMenu";
import { useEffect, useRef, useState } from "react";
import { providerLabel } from "./agentPresentation";
import { AgentPickerTrigger } from "./AgentPickerTrigger";
import { ReviewEmptyState } from "./ReviewEmptyState";
import { LoadingStatus } from "./LoadingStatus";
import { InlineError } from "./InlineError";
import { userFacingError } from "./userFacingError";
import { TerminalPane } from "./TerminalDock";
import { projectRunClient, type ProjectRunApi, type ProjectRunCatalog, type ProjectRunCommand, type ProjectRunEdit, type RunProcess } from "./projectRunClient";
import "./projectRun.css";
import { terminalEnded } from "./terminalClient";

type Execution = { key: string; directoryIndex: number; path: string; command: ProjectRunCommand; process: RunProcess; projectId: string; exited: boolean; uncertain?: boolean; hidden?: boolean };
const directoryName = (path: string) => path.replace(/[\\/]+$/, "").split(/[\\/]/).pop() || path;
export function ProjectRunPanel({ taskId, visible, api = projectRunClient, onEditProject }: { taskId: string; visible: boolean; api?: ProjectRunApi; onEditProject?: () => void }) {
  const [catalog, setCatalog] = useState<ProjectRunCatalog | null>(null);
  const [discoveredAgents, setAgents] = useState<Awaited<ReturnType<ProjectRunApi["executors"]>>>([]);
  const agents = useEnabledAgents(discoveredAgents);
  const [agent, setAgent] = useState("");
  const selectedAgent = (agent ? agents.find(value => agentChoiceKey(value) === agentChoiceKey(JSON.parse(agent))) : undefined) ?? agents[0];
  const [agentMenu, setAgentMenu] = useState(false);
  const agentPicker = useRef<HTMLDivElement>(null);
  const initializedAgentTask = useRef<string | null>(null);
  useEffect(() => {
    if (!agentMenu) return;
    const close = (event: PointerEvent) => { if (!agentPicker.current?.contains(event.target as Node)) setAgentMenu(false); };
    const escape = (event: KeyboardEvent) => { if (event.key === "Escape") { setAgentMenu(false); agentPicker.current?.querySelector("button")?.focus(); } };
    document.addEventListener("pointerdown", close); document.addEventListener("keydown", escape);
    return () => { document.removeEventListener("pointerdown", close); document.removeEventListener("keydown", escape); };
  }, [agentMenu]);
  useEffect(() => { setAgentMenu(false); }, [taskId, visible]);
  const [confirmGeneration, setConfirmGeneration] = useState<string | null>(null);
  useEffect(() => { if (!visible) setConfirmGeneration(null); }, [visible]);
  const [error, setError] = useState("");
  const [loadError, setLoadError] = useState("");
  useEffect(() => { setError(""); setLoadError(""); }, [taskId]);
  const [busy, setBusy] = useState(false);
  const [revision, setRevision] = useState(0);
  const [executions, setExecutions] = useState<Execution[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [resourcesLoaded, setResourcesLoaded] = useState(false);
  const mounted = useRef(true);
  const isVisible = useRef(visible);
  isVisible.current = visible;
  const request = useRef(0);
  const submitting = useRef(false);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; request.current++; }; }, []);
  useEffect(() => {
    if (!visible) return;
    let valid = true, loading = false, dirty = false;
    void api.executors().then(value => { if (valid) setAgents(value.filter(executor => executor.providerId !== "fake")); }).catch(() => {});
    let timer: ReturnType<typeof setTimeout>;
    const load = async () => {
      if (!valid) return;
      clearTimeout(timer);
      if (loading) { dirty = true; return; }
      if (submitting.current) { timer = setTimeout(load, 1200); return; }
      loading = true; dirty = false;
      let delay = 10000;
      const token = ++request.current;
      try {
        const value = await api.get(taskId);
        const resources = await api.list(value.projectId);
        if (!valid) return;
        if (token !== request.current || dirty) { delay = 0; return; }
        setCatalog(value); setLoadError(""); setResourcesLoaded(true);
        setExecutions(items => {
          const next = [...items];
          for (const info of resources.filter(info => info.taskId === taskId && info.commandId)) {
            const existing = next.findIndex(item => item.key === info.id);
            if (existing >= 0) { next[existing] = { ...next[existing], exited: terminalEnded(info), uncertain: info.status === "unknown" }; continue; }
            if (terminalEnded(info)) continue;
            next.push({ key: info.id, directoryIndex: info.directoryIndex, path: info.cwd, projectId: info.projectId,
              command: { id: info.commandId!, label: info.label, command: info.command ?? "", cwd: info.cwd, source: "restored" },
              exited: false, uncertain: info.status === "unknown", process: api.restore(info, () => {
                if (mounted.current) setExecutions(current => current.map(item => item.key === info.id ? { ...item, exited: true } : item));
              }),
            });
          }
          return next;
        });
        if (initializedAgentTask.current !== taskId) {
          initializedAgentTask.current = taskId;
          setAgent(value.executor && value.executor.providerId !== "fake" ? JSON.stringify(value.executor) : "");
        }
        delay = value.directories.some(d => d.status === "analyzing") ? 1200 : 10000;
      } catch (cause) { if (valid) { if (token === request.current) { setLoadError(userFacingError(cause, "无法读取运行命令，请稍后重试。")); setResourcesLoaded(false); } } }
      finally { loading = false; if (valid) timer = setTimeout(load, dirty ? 0 : delay); }
    };
    const stop = api.watch(taskId, () => { void load(); }, message => { if (valid) setLoadError(message); });
    const focus = () => { void load(); };
    window.addEventListener("focus", focus);
    void load();
    return () => { valid = false; stop(); window.removeEventListener("focus", focus); clearTimeout(timer); request.current++; };
  }, [taskId, visible, api, revision]);
  const pendingOperation = api.hasPending(taskId);
  const analyze = async (overwriteGeneration?: string) => {
    if (submitting.current || !selectedAgent || !catalog || pendingOperation) return;
    submitting.current = true; setBusy(true); setError(""); request.current++;
    try {
      const value = await api.analyze(taskId, selectedAgent, overwriteGeneration, catalog.generation);
      if (mounted.current) { setCatalog(value); }
    } catch (cause) {
      if (cause instanceof RunOperationRejected && cause.code === "confirmation_required") {
        try { const fresh = await api.get(taskId); if (mounted.current) { setCatalog(fresh); if (isVisible.current) setConfirmGeneration(fresh.generation); } }
        catch (error) { if (mounted.current) setError(userFacingError(error, "无法读取最新的运行命令，请重试。")); }
      } else if (mounted.current) setError(userFacingError(cause, "分析运行命令失败，请稍后重试。"));
    }
    finally { submitting.current = false; if (mounted.current) { setBusy(false); setRevision(n => n + 1); } }
  };
  const edit = async (directoryIndex: number, commandId: string, change: ProjectRunEdit): Promise<boolean> => {
    if (!catalog || submitting.current || pendingOperation) return false;
    submitting.current = true; setBusy(true); setError(""); request.current++;
    try {
      const value = await api.edit(taskId, catalog.generation, directoryIndex, commandId, change);
      if (mounted.current) {
        setCatalog(value);
        if (change.action === "rename") setExecutions(items => items.map(item => item.command.id === commandId ? { ...item, command: { ...item.command, label: change.label } } : item));
      }
      return true;
    } catch (cause) {
      if (cause instanceof RunOperationRejected && cause.code === "version_conflict") {
        try { const fresh = await api.get(taskId); if (mounted.current) setCatalog(fresh); }
        catch (refreshError) { if (mounted.current) setLoadError(userFacingError(refreshError, "无法刷新运行命令，请稍后重试。")); }
      }
      if (mounted.current) setError(userFacingError(cause, "保存运行命令失败，请重试。"));
      return false;
    }
    finally { submitting.current = false; if (mounted.current) setBusy(false); }
  };
  const requestAnalysis = () => {
    if (catalog?.directories.some(directory => directory.userModified)) setConfirmGeneration(catalog.generation);
    else void analyze();
  };
  const run = (command: ProjectRunCommand, directoryIndex: number) => {
    if (!catalog || !resourcesLoaded) return;
    const existing = executions.find(item => item.command.id === command.id && !item.exited);
    if (existing) { setExecutions(items => items.map(item => item.key === existing.key ? { ...item, hidden: false } : item)); setSelected(existing.key); return; }
    let key = "";
    const process = api.launch(taskId, catalog, directoryIndex, command.id, () => {
      if (mounted.current) setExecutions(items => items.map(item => item.key === key ? { ...item, exited: true } : item));
    });
    key = process.id;
    setExecutions(items => [...items, { key, directoryIndex, path: catalog.directories[directoryIndex].path, command, process, projectId: catalog.projectId, exited: false }]);
    setSelected(key);
  };
  const outputs = executions.filter(item => !item.hidden);
  useEffect(() => { if (!outputs.some(item => item.key === selected)) setSelected(outputs[0]?.key ?? null); }, [executions, selected]);
  const analyzing = busy || pendingOperation || catalog?.directories.some(d => d.status === "analyzing");
  const emptyCatalog = catalog && !error && !loadError && !analyzing
    && catalog.directories.every(directory => directory.status === "ready" && !directory.error && directory.commands.length === 0);
  const hasDirectories = Boolean(catalog?.directories.length);
  const canAnalyze = hasDirectories && !analyzing && Boolean(selectedAgent);
  const emptyDescription = !hasDirectories ? "为当前项目添加目录后，即可分析启动方式。"
    : !selectedAgent ? "连接可用的 Agent 后，即可分析项目的启动方式。"
    : catalog?.directories.some(directory => directory.userModified) ? "运行项已清空，可以重新分析项目生成。"
    : "未找到适合当前系统的运行命令。";
  return <div className="project-run-panel">
    {confirmGeneration && <RunAnalysisConfirmation onCancel={() => setConfirmGeneration(null)} onConfirm={() => { const generation = confirmGeneration; setConfirmGeneration(null); void analyze(generation); }} />}
    <div className="project-run-controls">
      {executions.some(item => item.hidden) && <button type="button" onClick={() => setExecutions(items => items.map(item => ({ ...item, hidden: false })))}>恢复运行输出</button>}
      <span className="project-run-label">使用 Agent 分析项目启动/运行方式</span>
      <div className="project-run-agent" ref={agentPicker}>
        <AgentPickerTrigger providerId={selectedAgent?.providerId} label="运行命令分析 Agent"
          tooltip={selectedAgent ? `运行命令分析 Agent：${providerLabel(selectedAgent.providerId)}` : agent ? "已选 Agent（当前不可用）" : "暂无可用 Agent，请先连接或重新检测 Agent"}
          open={agentMenu} disabled={busy} onClick={() => setAgentMenu(open => !open)}/>
        {agentMenu && <AgentPickerMenu options={agents.map(executor => ({ key: agentChoiceKey(executor), executor }))}
          selectedKey={agentChoiceKey(selectedAgent)} title="选择进行命令分析的 Agent" label="运行命令 Agent 列表" placement="bottom"
          onSelect={option => { setAgent(JSON.stringify(option.executor)); setAgentMenu(false); agentPicker.current?.querySelector("button")?.focus(); }}/>} 
      </div>
      <button type="button" className="turn-action project-run-refresh" aria-label="重新分析运行命令" data-tooltip="重新分析" disabled={!canAnalyze} onClick={requestAnalysis}><svg viewBox="0 0 24 24" aria-hidden="true"><path d="M20 7v5h-5M4 17v-5h5M6.1 6.1A8 8 0 0 1 20 12M4 12a8 8 0 0 0 13.9 5.9" /></svg></button>
    </div>
    {pendingOperation && <p role="status">上次操作的结果尚未核实。<button disabled={busy} onClick={() => {
      if (submitting.current) return;
      submitting.current = true; setBusy(true); request.current++;
      void api.retryPending(taskId).then(value => { if (mounted.current) { setCatalog(value); setError(""); } })
        .catch(cause => { if (mounted.current) setError(userFacingError(cause, "重试原操作失败，请稍后再试。")); })
        .finally(() => { submitting.current = false; if (mounted.current) { setBusy(false); setRevision(n => n + 1); } });
    }}>重新核实／重试原操作</button><button disabled={busy} data-tooltip="仅放弃重试，不会取消后台已开始的操作" onClick={() => { try { api.discardPending(taskId); setError(""); setRevision(n => n + 1); } catch (cause) { setError(userFacingError(cause, "放弃重试失败，请稍后再试。")); } }}>放弃重试</button></p>}
    {error && <InlineError>{error}</InlineError>}
    {loadError && <InlineError onRetry={() => setRevision(n => n + 1)}>{loadError}</InlineError>}
    {!catalog && !error && !loadError && <LoadingStatus layout="panel">正在读取运行命令…</LoadingStatus>}
    <div className={`project-run-directories${emptyCatalog ? " project-run-directories--empty" : ""}`}>
      {emptyCatalog ? <ReviewEmptyState kind="run" title={hasDirectories ? "暂无运行命令" : "尚未关联项目目录"}
        description={emptyDescription} compact={executions.length > 0}
        action={canAnalyze ? <button type="button" className="review-empty-action" onClick={requestAnalysis}>重新分析</button> : !hasDirectories && onEditProject ? <button type="button" className="review-empty-action" onClick={onEditProject}>添加项目目录</button> : undefined} /> : catalog?.directories.map((directory, index) => <section key={directory.path} className="project-run-directory" aria-label={directory.path}>
        <h3 data-tooltip={directory.path}>{directoryName(directory.path)}</h3>
        {directory.status === "analyzing" && <LoadingStatus className="project-run-analyzing">正在分析运行方式…</LoadingStatus>}
        {directory.error && <p role="alert">{directory.error}</p>}
        {directory.status === "ready" && !directory.error && directory.commands.length === 0 && <ReviewEmptyState kind="run" compact
          description={directory.userModified ? "暂无运行项，可重新分析生成。" : "未找到适合当前系统的运行命令"} />}
        {[...directory.commands].sort((a, b) => Number(Boolean(b.pinned)) - Number(Boolean(a.pinned))).map(command => <ProjectRunItem key={command.id} command={command} path={directory.path}
          disabled={Boolean(analyzing) || !resourcesLoaded} running={executions.some(item => item.command.id === command.id && !item.exited && !item.uncertain)}
          onRun={() => run(command, index)} onEdit={change => edit(index, command.id, change)} />)}
      </section>)}
    </div>
    {outputs.length > 0 && <section className="project-run-output" aria-label="运行输出">
      <div className="project-run-output-tabs" role="tablist" aria-label="运行中的命令">{outputs.map((item, index) => <div className="project-run-output-tab" role="presentation" key={item.key} data-selected={selected === item.key}>
        <button role="tab" type="button" aria-selected={selected === item.key} data-tooltip={`${item.path}\n${item.command.command}`} onClick={() => setSelected(item.key)}>{item.command.label}{item.exited ? " · 已结束" : item.uncertain ? " · 状态待核实" : ""}</button>
        <button type="button" className="project-run-tab-close" aria-label={`关闭输出 ${item.command.label}`} data-tooltip="隐藏输出，进程继续运行" onClick={() => {
          const remaining = outputs.filter(value => value.key !== item.key);
          setExecutions(items => items.map(value => value.key === item.key ? { ...value, hidden: true } : value)); if (selected === item.key) setSelected(remaining[Math.min(index, remaining.length - 1)]?.key ?? null);
        }}><svg viewBox="0 0 16 16" aria-hidden="true"><path d="m4 4 8 8M12 4l-8 8" /></svg></button>
      </div>)}</div>
      {outputs.map(item => <div className="project-run-console" key={item.key} hidden={selected !== item.key}>
        <div className="project-run-console-bar"><code data-tooltip={item.command.command}>{item.command.command}</code>
          {!item.exited && <button type="button" className="project-run-stop" aria-label="停止" data-tooltip="停止运行" onClick={() => void item.process.stop().catch(cause => setError(userFacingError(cause, "无法停止运行，请重试。")))}><svg viewBox="0 0 24 24" aria-hidden="true"><rect x="6" y="6" width="12" height="12" rx="2" /></svg></button>}
        </div>
        <TerminalPane operationId={item.key} projectId={item.projectId} directoryIndex={item.directoryIndex} visible={visible && selected === item.key} api={item.process.api} />
      </div>)}
    </section>}
  </div>;
}
