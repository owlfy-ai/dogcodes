import { LoadingStatus } from "./LoadingStatus";
import { InlineError } from "./InlineError";
import { userFacingError } from "./userFacingError";
import { ReviewEmptyState } from "./ReviewEmptyState";
import { useCallback, useEffect, useRef, useState } from "react";
import { terminalClient, terminalEnded, type TerminalApi, type TerminalConnection, type TerminalOutput, type TerminalStatus } from "./terminalClient";
export { terminalClient, type TerminalApi } from "./terminalClient";
import { Terminal } from "@xterm/xterm";
import { installTerminalKeyboard } from "./terminalKeyboard";
import { terminalInput } from "./terminalInput";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import "./terminal.css";
import { FolderIcon } from "./ProjectHoverCard";

export function TerminalPane({ projectId, directoryIndex, visible, api, resourceId, operationId, onResource, onStatus }: {
  projectId: string; directoryIndex: number; visible: boolean; api: TerminalApi; resourceId?: string; operationId?: string; onResource?: (id: string) => void; onStatus?: (status: TerminalStatus) => void;
}) {
  const host = useRef<HTMLDivElement>(null);
  const runtime = useRef<{ fit: () => void; focus: () => void } | null>(null);
  const isVisible = useRef(visible);
  isVisible.current = visible;
  const [status, setStatus] = useState("正在启动终端…");
  const [starting, setStarting] = useState(true);
  const [generation, setGeneration] = useState(0);
  const launch = useRef({ id: operationId ?? crypto.randomUUID(), cols: 0, rows: 0 });
  const knownResource = useRef(resourceId);
  const [retry, setRetry] = useState(false);
  const [failure, setFailure] = useState(0);
  const reconnect = useRef({ retry: false, delay: 1000 });
  reconnect.current.retry = retry;
  const resourceListener = useRef(onResource);
  resourceListener.current = onResource;
  const statusListener = useRef(onStatus);
  statusListener.current = onStatus;
  useEffect(() => {
    const element = host.current!;
    let disposed = false, exited = false, id: string | undefined, connection: TerminalConnection | undefined;
    setStatus(knownResource.current ? "正在连接终端…" : "正在启动终端…"); setStarting(true); setRetry(false);
    const terminal = new Terminal({
      cursorBlink: true, fontSize: 13, lineHeight: 1.25, scrollback: 5000,
      fontFamily: 'Menlo, Monaco, Consolas, "Liberation Mono", monospace',
      allowProposedApi: false,
      overviewRuler: { width: 10 },
    });
    const fitAddon = new FitAddon();
    terminal.loadAddon(fitAddon);
    terminal.open(element);
    const keyboardCleanup = installTerminalKeyboard(terminal, element);
    const theme = () => {
      const style = getComputedStyle(element);
      const foreground = style.getPropertyValue("--terminal-fg").trim();
      terminal.options.theme = {
        background: style.getPropertyValue("--terminal-bg").trim(),
        foreground: foreground && foreground !== "currentColor" ? foreground : style.color,
        cursor: foreground && foreground !== "currentColor" ? foreground : style.color,
        selectionBackground: "#6688bb66",
        scrollbarSliderBackground: style.getPropertyValue("--scrollbar-thumb").trim(),
        scrollbarSliderHoverBackground: style.getPropertyValue("--scrollbar-thumb-hover").trim(),
        scrollbarSliderActiveBackground: style.getPropertyValue("--scrollbar-thumb-hover").trim(),
      };
    };
    theme();
    const report = (error: unknown) => {
      if (!disposed && !exited) { setStarting(false); setStatus(userFacingError(error, "终端出现错误，请重新打开。")); }
    };
    let lastSize = "";
    const fit = () => {
      if (disposed || !element.clientWidth || !element.clientHeight) return;
      const scale = Number.parseFloat(getComputedStyle(element).getPropertyValue("--ui-scale")) || 1;
      const fontSize = 13 * scale;
      if (terminal.options.fontSize !== fontSize) terminal.options.fontSize = fontSize;
      fitAddon.fit();
      const nextSize = `${terminal.cols}:${terminal.rows}`;
      if (id && !exited && lastSize !== nextSize) {
        lastSize = nextSize;
        void api.resize(id, terminal.cols, terminal.rows).catch(report);
      }
    };
    const observer = new ResizeObserver(fit);
    observer.observe(element);
    const themeObserver = new MutationObserver(() => { theme(); fit(); });
    themeObserver.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme", "style"] });
    const media = window.matchMedia("(prefers-color-scheme: light)");
    media.addEventListener("change", theme);
    runtime.current = { fit, focus: () => terminal.focus() };
    fit();
    const inputQueue = terminalInput(api.write, error => {
      if (disposed || exited) return;
      exited = true;
      terminal.options.disableStdin = true;
      setStarting(false); setRetry(true); setFailure(n => n + 1); setStatus("输入结果无法确认，未重新发送。");
      connection?.detach();
    });
    const send = (bytes: Uint8Array) => { if (!exited && !disposed) inputQueue.push(bytes); };
    const input = terminal.onData(data => send(new TextEncoder().encode(data)));
    const binary = terminal.onBinary(data => send(Uint8Array.from(data, c => c.charCodeAt(0))));
    const receive = (event: TerminalOutput) => {
      if (disposed) return;
      if (event.kind === "data") terminal.write(new Uint8Array(event.bytes));
      else if (event.kind === "gap") terminal.write("\r\n[部分历史输出已不可用]\r\n");
      else if (event.kind === "disconnected") {
        inputQueue.dispose(); exited = true; terminal.options.disableStdin = true;
        setStarting(false); setRetry(true); setFailure(n => n + 1); setStatus(userFacingError(event.error, "终端连接已中断，进程状态待核实。"));
      } else {
        const info = event.info;
        setStarting(false);
        statusListener.current?.(info.status);
        terminal.options.disableStdin = info.status !== "running";
        if (info.status === "running") { setStatus(""); }
        else if (info.status === "starting") setStatus("正在启动终端…");
        else if (info.status === "stopping") setStatus("正在停止…");
        else {
          inputQueue.dispose(); exited = true; terminal.options.disableStdin = true; setRetry(false);
          setStatus(info.status === "unknown" ? (info.error ?? "原进程状态无法确认，未自动重新执行")
            : info.status === "failed" ? (info.error ?? "终端启动失败") : `终端已退出${info.exitCode === null ? "" : `（${info.exitCode}）`}`);
        }
      }
    };
    // StrictMode's discarded mount must not start a real shell.
    void Promise.resolve().then(async () => {
      if (disposed) return;
      if (knownResource.current) return api.attach(knownResource.current, receive);
      const intent = launch.current;
      if (!intent.cols) { intent.cols = terminal.cols; intent.rows = terminal.rows; }
      // Remember the identity before sending: a lost reply must only reconnect,
      // never launch another shell or replay terminal input.
      knownResource.current = intent.id;
      resourceListener.current?.(intent.id);
      return api.open(projectId, directoryIndex, intent.cols, intent.rows, receive, intent.id);
    }).then(attached => {
      if (!attached) return;
      connection = attached;
      if (disposed) { attached.detach(); return; }
      id = attached.id; knownResource.current = id; resourceListener.current?.(id); setStarting(false);
      if (!exited) { reconnect.current.delay = 1000; inputQueue.ready(id); }
      fit();
      if (isVisible.current && element.clientWidth) terminal.focus();
    }).catch(error => { if (!disposed) { inputQueue.dispose(); setStarting(false); setRetry(true); setFailure(n => n + 1); setStatus(userFacingError(error, "无法连接终端，执行状态待核实。")); } });
    return () => {
      disposed = true;
      inputQueue.dispose();
      runtime.current = null;
      observer.disconnect(); themeObserver.disconnect(); media.removeEventListener("change", theme);
      keyboardCleanup(); input.dispose(); binary.dispose(); terminal.dispose();
      connection?.detach();
    };
  }, [projectId, directoryIndex, generation, api]);
  useEffect(() => {
    if (!visible || !retry) return;
    const timer = setTimeout(() => setGeneration(n => n + 1), reconnect.current.delay);
    reconnect.current.delay = Math.min(reconnect.current.delay * 2, 15000);
    return () => clearTimeout(timer);
  }, [visible, retry, failure]);
  useEffect(() => {
    if (!visible) return;
    runtime.current?.fit(); runtime.current?.focus();
    if (reconnect.current.retry) setGeneration(n => n + 1);
  }, [visible]);
  return <div className="terminal-pane" role="tabpanel" hidden={!visible}>
    {status && <div className="terminal-status" role={starting || retry ? undefined : "status"}>{starting ? <LoadingStatus>{status}</LoadingStatus>
      : retry ? <InlineError>{status} 正在自动重新连接…</InlineError> : status}</div>}
    <div className="terminal-emulator" ref={host} />
  </div>;
}

export function TerminalDock({ projectId, directories, open, onClose, api = terminalClient, onEditProject }: {
  projectId: string; directories: string[]; open: boolean; onClose: () => void; api?: TerminalApi; onEditProject?: () => void;
}) {
  const [opened, setOpened] = useState(open);
  type Tab = { id: string; directoryIndex: number; ordinal: number; resourceId?: string; status?: TerminalStatus; hidden?: boolean; closed?: boolean };
  const [selected, setSelected] = useState("");
  const [tabs, setTabs] = useState<Tab[]>([]);
  const latestTabs = useRef(tabs);
  latestTabs.current = tabs;
  const [loaded, setLoaded] = useState(false);
  const [loadError, setLoadError] = useState("");
  const [closeError, setCloseError] = useState("");
  const [closing, setClosing] = useState(false);
  const closingRequest = useRef(false);
  const mounted = useRef(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  const [refresh, setRefresh] = useState(0);
  useEffect(() => {
    if (!open) return;
    let valid = true;
    void api.list(projectId).then(resources => {
      if (!valid) return;
      const shells = resources.filter(info => info.taskId === null);
      setTabs(current => {
        const next = [...current];
        for (const info of shells) {
          if (next.some(tab => tab.id === info.id || tab.resourceId === info.id) || terminalEnded(info)) continue;
          next.push({ id: info.id, resourceId: info.id, status: info.status, directoryIndex: info.directoryIndex,
            ordinal: 1 + next.filter(tab => tab.directoryIndex === info.directoryIndex).length });
        }
        if (!loaded && shells.length === 0 && current.length === 0) {
          directories.forEach((_, directoryIndex) => next.push({ id: crypto.randomUUID(), directoryIndex, ordinal: 1 }));
        }
        return next;
      });
      setLoaded(true); setLoadError("");
    }).catch(error => { if (valid) setLoadError(userFacingError(error, "无法查询后台终端，请重试。")); });
    return () => { valid = false; };
    // Directory changes must not relaunch existing shells. A new shell always
    // resolves the selected root through the backend at creation time.
  }, [projectId, open, api, refresh]);
  const rememberResource = useCallback((tabId: string, resourceId: string) => {
    setTabs(current => current.map(tab => tab.id === tabId && tab.resourceId !== resourceId ? { ...tab, resourceId } : tab));
  }, []);
  const rememberStatus = useCallback((tabId: string, status: TerminalStatus) => {
    setTabs(current => current.map(tab => tab.id === tabId && tab.status !== status ? { ...tab, status } : tab));
  }, []);
  const visibleTabs = tabs.filter(tab => !tab.hidden);
  const hiddenTabs = tabs.filter(tab => tab.hidden && !tab.closed);
  const hasTerminalMenu = directories.length > 1 || hiddenTabs.length > 0;
  useEffect(() => {
    if (!visibleTabs.some(tab => tab.id === selected)) setSelected(visibleTabs[0]?.id ?? "");
  }, [tabs, selected]);
  const [menuOpen, setMenuOpen] = useState(false);
  const addControl = useRef<HTMLDivElement>(null);
  const tabList = useRef<HTMLDivElement>(null);
  const directoryName = (index: number) => directories[index]?.replace(/[\\/]+$/, "").split(/[\\/]/).pop() || "工作区";
  const addTab = (directoryIndex: number) => {
    const id = crypto.randomUUID();
    setTabs(current => [...current, { id, directoryIndex, ordinal: Math.max(0, ...current.filter(tab => tab.directoryIndex === directoryIndex).map(tab => tab.ordinal)) + 1 }]);
    setSelected(id);
    setMenuOpen(false);
  };
  const requestTerminal = () => {
    if (!directories.length && !hiddenTabs.length) return;
    if (!hasTerminalMenu) addTab(0);
    else setMenuOpen(value => !value);
  };
  const closeTab = (id: string, closed = false) => {
    const visible = latestTabs.current.filter(tab => !tab.hidden);
    const index = visible.findIndex(tab => tab.id === id);
    const remaining = visible.filter(tab => tab.id !== id);
    setTabs(current => current.map(tab => tab.id === id ? { ...tab, hidden: true, closed } : tab));
    setSelected(current => current === id ? remaining[Math.min(index, remaining.length - 1)]?.id ?? "" : current);
    if (!remaining.length) addControl.current?.querySelector("button")?.focus();
  };
  const currentTerminal = visibleTabs.find(tab => tab.id === selected);
  const closeCurrentTerminal = async () => {
    const tab = currentTerminal;
    if (!tab?.resourceId || !tab.status || closingRequest.current) return;
    closingRequest.current = true; setClosing(true); setCloseError("");
    try {
      await api.close(tab.resourceId);
      if (!mounted.current) return;
      closeTab(tab.id, true);
      if (!latestTabs.current.some(item => !item.hidden && item.id !== tab.id)) onClose();
    } catch (cause) {
      if (mounted.current) setCloseError(userFacingError(cause, "无法关闭当前终端，请重试。"));
    } finally {
      closingRequest.current = false;
      if (mounted.current) setClosing(false);
    }
  };
  const tabLabel = (tab: typeof tabs[number]) => directoryName(tab.directoryIndex) + (tab.ordinal > 1 ? ` · ${tab.ordinal}` : "");
  useEffect(() => { if (!open) setMenuOpen(false); }, [open]);
  useEffect(() => {
    tabList.current?.querySelector('[aria-selected="true"]')?.scrollIntoView?.({ block: "nearest", inline: "nearest" });
  }, [selected, open]);
  useEffect(() => {
    if (!menuOpen) return;
    addControl.current?.querySelector<HTMLButtonElement>('[role="menuitem"]')?.focus();
    const outside = (event: PointerEvent) => { if (!addControl.current?.contains(event.target as Node)) setMenuOpen(false); };
    const escape = (event: KeyboardEvent) => {
      if (event.key === "Escape") { setMenuOpen(false); addControl.current?.querySelector("button")?.focus(); }
    };
    document.addEventListener("pointerdown", outside);
    document.addEventListener("keydown", escape);
    return () => { document.removeEventListener("pointerdown", outside); document.removeEventListener("keydown", escape); };
  }, [menuOpen]);
  const [height, setHeight] = useState(260);
  const drag = useRef<{ y: number; height: number } | null>(null);
  useEffect(() => { if (open) setOpened(true); }, [open]);
  if (!opened) return null;
  return <section className="terminal-panel" aria-label="项目终端" hidden={!open} style={{ height }}>
    <div className="terminal-resize" role="separator" aria-label="调整终端高度" aria-orientation="horizontal" tabIndex={0}
      aria-valuemin={140} aria-valuemax={650} aria-valuenow={height}
      onKeyDown={e => { if (e.key === "ArrowUp" || e.key === "ArrowDown") { e.preventDefault(); setHeight(h => Math.min(650, Math.max(140, h + (e.key === "ArrowUp" ? 20 : -20)))); } }}
      onPointerDown={e => { e.preventDefault(); drag.current = { y: e.clientY, height }; e.currentTarget.setPointerCapture(e.pointerId); }}
      onPointerMove={e => { if (drag.current) setHeight(Math.min(650, Math.max(140, drag.current.height + drag.current.y - e.clientY))); }}
      onPointerUp={() => { drag.current = null; }} onPointerCancel={() => { drag.current = null; }} />
    <header className="terminal-toolbar"><strong>终端</strong><div ref={tabList} role="tablist" aria-label="项目目录终端">
      {visibleTabs.map((tab, index) => <div className="terminal-tab" role="presentation" key={tab.id} data-selected={selected === tab.id}><button role="tab" aria-selected={selected === tab.id}
        onClick={() => setSelected(tab.id)} onKeyDown={e => {
          if (e.key === "ArrowRight" || e.key === "ArrowLeft") {
            e.preventDefault();
            const next = (index + (e.key === "ArrowRight" ? 1 : visibleTabs.length - 1)) % visibleTabs.length;
            setSelected(visibleTabs[next].id);
            tabList.current?.querySelectorAll<HTMLButtonElement>('[role="tab"]')[next]?.focus();
          }
        }} tabIndex={selected === tab.id ? 0 : -1}>{tabLabel(tab)}</button>
        <button className="terminal-tab-close" aria-label={`关闭终端 ${tabLabel(tab)}`} data-tooltip="隐藏输出，进程继续运行" onClick={() => closeTab(tab.id)}><svg viewBox="0 0 16 16" aria-hidden="true"><path d="m4 4 8 8M12 4l-8 8" /></svg></button>
      </div>)}
    </div>
    <div className="terminal-add-control" ref={addControl}>
      <button className="terminal-add" aria-label="新建终端" data-tooltip="新建终端" disabled={(!directories.length && !hiddenTabs.length) || !loaded}
        aria-haspopup={hasTerminalMenu ? "menu" : undefined} aria-expanded={hasTerminalMenu ? menuOpen : undefined}
        onClick={requestTerminal}>+</button>
      {menuOpen && <div className="terminal-directory-menu" role="menu" aria-label="选择终端目录" onKeyDown={e => {
        if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
        e.preventDefault();
        const items = Array.from(e.currentTarget.querySelectorAll<HTMLButtonElement>('[role="menuitem"]'));
        const index = items.indexOf(document.activeElement as HTMLButtonElement);
        items[(index + (e.key === "ArrowDown" ? 1 : items.length - 1)) % items.length]?.focus();
      }}>
        {hiddenTabs.length > 0 && <><span>已有终端</span>{hiddenTabs.map(tab => <button key={tab.id} role="menuitem" onClick={() => {
          setTabs(current => current.map(item => item.id === tab.id ? { ...item, hidden: false } : item));
          setSelected(tab.id); setMenuOpen(false);
        }}><FolderIcon />{tabLabel(tab)}</button>)}</>}
        <span>{hiddenTabs.length ? "新建终端" : "选择终端目录"}</span>
        {directories.map((_, index) => <button key={index} role="menuitem" onClick={() => addTab(index)}>
          <FolderIcon />{hiddenTabs.length ? `新建 ${directoryName(index)}` : directoryName(index)}
        </button>)}
      </div>}
    </div>
    <button className="terminal-close" aria-label="关闭当前终端" data-tooltip={closing ? "正在关闭当前终端…" : "关闭当前终端"} disabled={closing || !currentTerminal?.resourceId || !currentTerminal.status} onClick={() => void closeCurrentTerminal()}>×</button></header>
    <div className="terminal-panes">{closeError && <InlineError>{closeError}</InlineError>}{loadError && <InlineError onRetry={() => setRefresh(n => n + 1)}>{loadError}</InlineError>}
    {!loaded && !loadError && <LoadingStatus>正在查询后台终端…</LoadingStatus>}
    {loaded && visibleTabs.length === 0 && <div className="terminal-empty"><ReviewEmptyState kind="terminal" compact title={directories.length ? "暂无终端" : "尚未关联项目目录"}
      description={directories.length ? "新建终端，在项目目录中运行命令。" : "先为项目添加目录，再创建终端。"}
      action={directories.length ? <button type="button" className="review-empty-action" aria-haspopup={hasTerminalMenu ? "menu" : undefined} aria-expanded={hasTerminalMenu ? menuOpen : undefined} onClick={requestTerminal}>新建终端</button> : onEditProject && <button type="button" className="review-empty-action" onClick={onEditProject}>添加项目目录</button>} /></div>}{visibleTabs.map(tab => <TerminalPane key={tab.id} operationId={tab.id} resourceId={tab.resourceId} onResource={id => rememberResource(tab.id, id)} onStatus={status => rememberStatus(tab.id, status)} projectId={projectId} directoryIndex={tab.directoryIndex} visible={open && selected === tab.id} api={api} />)}</div>
  </section>;
}
