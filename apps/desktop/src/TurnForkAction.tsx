import { userFacingError } from "./userFacingError";
import { useEffect, useId, useRef, useState, useSyncExternalStore } from "react";
import { ulid } from "ulid";
import type { CoreClient, ForkEnvironment, ForkReceipt, ForkRequest, TaskId } from "@dogcodes/protocol";
import { ActionIcon } from "./ReviewActions";
import { FloatingPopover } from "./FloatingPopover";
import { forkOperations } from "./forkOperations";
import { LoadingStatus } from "./LoadingStatus";

const options: { environment: ForkEnvironment; label: string; description: string }[] = [
  { environment: "workspace", label: "在当前工作区 Fork", description: "从本轮对话继续，沿用当前工作区" },
  { environment: "worktree", label: "在新 Worktree 中 Fork", description: "恢复本轮结束时的文件，在新 worktree 中继续" },
];

export function TurnForkAction({ client, taskId, turnId, onForked }: {
  client: CoreClient; taskId: TaskId; turnId: string; onForked?: (taskId: TaskId) => Promise<void> | void;
}) {
  const [open, setOpen] = useState(false), [busy, setBusy] = useState(false), [error, setError] = useState("");
  const host = useRef<HTMLDivElement>(null), trigger = useRef<HTMLButtonElement>(null);
  const saved = useSyncExternalStore(forkOperations.subscribe, () => forkOperations.snapshot(taskId, turnId), () => "null");
  const record = JSON.parse(saved) as ForkRequest | { error: string } | null;
  const pending = record && "newTaskId" in record ? record : null;
  const [missing, setMissing] = useState(false);
  const [receipt, setReceipt] = useState<ForkReceipt | null>(null);
  const context = useRef(""); context.current = `${taskId}:${turnId}`;
  useEffect(() => { setMissing(false); setReceipt(null); }, [saved]);
  const guard = useRef(false), mounted = useRef(true), menuId = useId();
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  const close = () => { setOpen(false); trigger.current?.focus({ preventScroll: true }); };
  useEffect(() => {
    if (!open) return;
    host.current?.querySelector<HTMLButtonElement>('[role="menuitem"]:not(:disabled)')?.focus();
    const outside = (event: PointerEvent) => { if (!host.current?.contains(event.target as Node)) setOpen(false); };
    const escape = (event: KeyboardEvent) => { if (event.key === "Escape") { event.preventDefault(); close(); } };
    document.addEventListener("pointerdown", outside); document.addEventListener("keydown", escape);
    return () => { document.removeEventListener("pointerdown", outside); document.removeEventListener("keydown", escape); };
  }, [open]);
  useEffect(() => {
    if (!open || !pending || !client.bootstrap || !client.subscribe) return;
    let valid = true, checking = false, dirty = false, stop: (() => void) | undefined;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let cursor = 0, epoch = 0;
    const refresh = async () => {
      if (!valid) return;
      if (checking) { dirty = true; return; }
      checking = true;
      try {
        const value = await forkOperations.check(client, pending);
        if (valid) { setReceipt(value); setMissing(!value?.completed); setError(""); }
      } catch (cause) { if (valid) setError(userFacingError(cause, "无法核实 Fork 状态，请稍后重试。")); }
      finally { checking = false; if (dirty && valid) { dirty = false; void refresh(); } }
    };
    const connect = async () => {
      if (!valid) return;
      const attempt = ++epoch;
      let ended = false;
      stop?.(); stop = undefined;
      try {
        if (!cursor) cursor = (await client.bootstrap()).lastSequence;
        if (!valid) return;
        const cancel = await client.subscribe(cursor, message => {
          if (!valid || attempt !== epoch || ended) return;
          if (message.type === "event") {
            cursor = Math.max(cursor, message.data.sequence);
            const payload = message.data.payload as Record<string, unknown>;
            if (payload.newTaskId === pending.newTaskId || payload.taskId === pending.newTaskId) void refresh();
          } else if (message.type === "error") {
            ended = true;
            stop?.(); stop = undefined; clearTimeout(timer);
            timer = setTimeout(() => { void connect(); }, 2000);
            void refresh();
          }
        });
        if (!valid || attempt !== epoch || ended) cancel(); else { stop = cancel; void refresh(); }
      } catch (cause) {
        if (valid && attempt === epoch) { setError(userFacingError(cause, "Fork 状态同步已断开，正在重连。")); timer = setTimeout(() => { void connect(); }, 2000); }
      }
    };
    const focus = () => { void refresh(); };
    window.addEventListener("focus", focus);
    void connect();
    return () => { valid = false; stop?.(); clearTimeout(timer); window.removeEventListener("focus", focus); };
  }, [client, open, saved]);
  async function operate(action: () => Promise<void>) {
    if (guard.current) return;
    guard.current = true; setBusy(true); setError("");
    const origin = `${taskId}:${turnId}`;
    try { await action(); }
    catch (cause) { if (mounted.current && context.current === origin) setError(userFacingError(cause, "Fork 操作未完成，请核实结果。")); }
    finally { guard.current = false; if (mounted.current) setBusy(false); }
  }
  async function opened(request: ForkRequest, id: TaskId) {
    if (!mounted.current || context.current !== `${request.taskId}:${request.turnId}`) return;
    await onForked?.(id);
    forkOperations.forget(request);
    if (mounted.current && context.current === `${request.taskId}:${request.turnId}`) close();
  }
  function fork(environment: ForkEnvironment) {
    if (!client.forkTask || guard.current || record) return;
    const request: ForkRequest = { taskId, turnId, newTaskId: `tsk_${ulid()}`, environment };
    void operate(async () => opened(request, await forkOperations.send(client, request)));
  }
  return <div className="turn-fork" ref={host}>
    <button type="button" className="turn-action" ref={trigger} aria-label="从此处 Fork 对话" data-tooltip="从此处 Fork 对话"
      aria-haspopup="menu" aria-expanded={open} aria-controls={open ? menuId : undefined} disabled={!client.forkTask || busy}
      onClick={() => setOpen(value => !value)} onKeyDown={event => {
        if (event.key === "ArrowDown" || event.key === "ArrowUp") { event.preventDefault(); setOpen(true); }
      }}><ActionIcon name="fork"/></button>
    {open && <FloatingPopover className="turn-fork-menu" id={menuId} role="menu" aria-label="从此处 Fork 对话" align="start" onKeyDown={event => {
      const buttons = [...event.currentTarget.querySelectorAll<HTMLButtonElement>('[role="menuitem"]:not(:disabled)')];
      const index = buttons.indexOf(document.activeElement as HTMLButtonElement);
      const next = event.key === "ArrowDown" ? (index + 1) % buttons.length : event.key === "ArrowUp" ? (index - 1 + buttons.length) % buttons.length : event.key === "Home" ? 0 : event.key === "End" ? buttons.length - 1 : null;
      if (next !== null) { event.preventDefault(); buttons[next]?.focus(); }
      if (event.key === "Tab") setOpen(false);
    }}>
      <header>从此处 Fork 对话</header>
      {options.map(option => <button type="button" role="menuitem" key={option.environment} disabled={busy || !!record}
        onClick={() => void fork(option.environment)}>
        <ActionIcon name="fork"/>
        <span><strong>{option.label}</strong><small>{option.description}</small></span>
      </button>)}
      {record && "error" in record && <p className="turn-fork-feedback" role="alert">{record.error}</p>}
      {pending && <div className="turn-fork-feedback" role="group" aria-label="待核实的 Fork 请求">
        <p>有一条 Fork 请求尚未确认打开：{pending.environment === "worktree" ? "新 Worktree" : "当前工作区"}。</p>
        <button type="button" role="menuitem" disabled={busy} onClick={() => void operate(async () => {
          const receipt = await forkOperations.check(client, pending);
          setReceipt(receipt);
          if (receipt?.completed) await opened(pending, forkOperations.available(receipt));
          else if (mounted.current && context.current === `${pending.taskId}:${pending.turnId}`) setMissing(true);
        })}>核实并打开 Fork</button>
        {receipt && <p>{receipt.status === "running" ? "Fork 正在后台执行，请等待。" : receipt.status === "succeeded" ? "Fork 已创建，可以打开。" : receipt.status === "failed" ? "Fork 未完成。" : "原 Fork 已中断。"}{receipt.error}</p>}
        {missing && receipt?.status !== "running" && <><p>后台尚无完成记录，Worktree 可能已部分创建。重试会保留原目标和环境。</p><button type="button" role="menuitem" disabled={busy} onClick={() => void operate(async () => opened(pending, await forkOperations.resend(client, pending)))}>重试原 Fork</button></>}
        <button type="button" role="menuitem" disabled={busy} onClick={() => { try { forkOperations.forget(pending); setError(""); } catch (cause) { setError(String(cause)); } }}>已核对，结束 Fork 跟踪</button>
        <small>结束跟踪不会取消后台操作或删除 Worktree。</small>
      </div>}
      {busy && <div className="turn-fork-hint"><LoadingStatus>正在创建 Fork…</LoadingStatus></div>}
      {error && <p className="turn-fork-feedback" role="alert">{error}</p>}
    </FloatingPopover>}
    {record && !open && <span className="turn-fork-feedback">有待核实的 Fork 请求，请打开 Fork 菜单查看。</span>}
    {busy && !open && <span className="turn-fork-feedback"><LoadingStatus>正在创建 Fork…</LoadingStatus></span>}
    {error && !open && <span className="turn-fork-feedback" role="alert">{error}</span>}
  </div>;
}
