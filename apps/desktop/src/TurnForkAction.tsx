import { userFacingError } from "./userFacingError";
import { useEffect, useId, useRef, useState, useSyncExternalStore } from "react";
import { ulid } from "ulid";
import type { CoreClient, ForkEnvironment, ForkReceipt, ForkRequest, TaskId } from "@dogcodes/protocol";
import { ActionIcon } from "./ReviewActions";
import { FloatingPopover } from "./FloatingPopover";
import { ForkPending, forkOperations } from "./forkOperations";
import { displayBackendText } from "./backendText";
import { LoadingStatus } from "./LoadingStatus";
import { translate as t } from "./i18n";

const options: { environment: ForkEnvironment; label: string; description: string }[] = [
  { environment: "workspace", label: "fork.workspace", description: "fork.workspaceDescription" },
  { environment: "worktree", label: "fork.worktree", description: "fork.worktreeDescription" },
];

export function TurnForkAction({ client, taskId, turnId, onForked }: {
  client: CoreClient; taskId: TaskId; turnId: string; onForked?: (taskId: TaskId) => Promise<void> | void;
}) {
  const [open, setOpen] = useState(false), [busy, setBusy] = useState(false), [error, setError] = useState("");
  const host = useRef<HTMLDivElement>(null), trigger = useRef<HTMLButtonElement>(null);
  const saved = useSyncExternalStore(forkOperations.subscribe, () => forkOperations.snapshot(taskId, turnId), () => "null");
  const record = JSON.parse(saved) as ForkRequest | { error: string } | null;
  const pending = record && "newTaskId" in record ? record : null;
  // What became of a kept fork: still running, or stopped and waiting on the person.
  const [receipt, setReceipt] = useState<ForkReceipt | null | undefined>(undefined);
  const [failure, setFailure] = useState("");
  const context = useRef(""); context.current = `${taskId}:${turnId}`;
  useEffect(() => { setReceipt(undefined); setFailure(""); }, [saved]);
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
  // A fork whose reply was lost is settled in the background from its receipt:
  // a finished one is dropped, a running one is followed until it ends.
  useEffect(() => {
    if (!pending || busy) return;
    let valid = true, checking = false, dirty = false, stop: (() => void) | undefined;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let cursor = 0, epoch = 0;
    const refresh = async () => {
      if (!valid) return;
      if (checking) { dirty = true; return; }
      checking = true;
      try {
        const value = await forkOperations.settle(client, pending);
        if (valid) setReceipt(value);
      } catch { /* checked again on the next change */ }
      finally { checking = false; if (dirty && valid) { dirty = false; void refresh(); } }
    };
    const connect = async () => {
      if (!valid) return;
      if (!client.bootstrap || !client.subscribe) { void refresh(); return; }
      const attempt = ++epoch;
      let ended = false;
      stop?.(); stop = undefined;
      try {
        if (!cursor) cursor = (await client.bootstrap!()).lastSequence;
        if (!valid) return;
        const cancel = await client.subscribe!(cursor, message => {
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
      } catch {
        if (valid && attempt === epoch) timer = setTimeout(() => { void connect(); }, 2000);
      }
    };
    const focus = () => { void refresh(); };
    window.addEventListener("focus", focus);
    void connect();
    return () => { valid = false; stop?.(); clearTimeout(timer); window.removeEventListener("focus", focus); };
  }, [client, saved, busy]);
  async function operate(action: () => Promise<void>) {
    if (guard.current) return;
    guard.current = true; setBusy(true); setError(""); setFailure("");
    const origin = `${taskId}:${turnId}`;
    try { await action(); }
    catch (cause) {
      if (!mounted.current || context.current !== origin) return;
      // A kept fork is reported by its own notice, with a retry.
      if (cause instanceof ForkPending) setFailure(userFacingError(cause, t("fork.incomplete")));
      else setError(userFacingError(cause, t("fork.incomplete")));
    }
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
  const running = !!pending && receipt?.status === "running";
  const stopped = !!pending && !busy && receipt !== undefined && !running;
  return <div className="turn-fork" ref={host}>
    <button type="button" className="turn-action" ref={trigger} aria-label={t("fork.title")} data-tooltip={t("fork.title")}
      aria-haspopup="menu" aria-expanded={open} aria-controls={open ? menuId : undefined} disabled={!client.forkTask || busy}
      onClick={() => setOpen(value => !value)} onKeyDown={event => {
        if (event.key === "ArrowDown" || event.key === "ArrowUp") { event.preventDefault(); setOpen(true); }
      }}><ActionIcon name="fork"/></button>
    {open && <FloatingPopover className="turn-fork-menu" id={menuId} role="menu" aria-label={t("fork.title")} align="start" onKeyDown={event => {
      const buttons = [...event.currentTarget.querySelectorAll<HTMLButtonElement>('[role="menuitem"]:not(:disabled)')];
      const index = buttons.indexOf(document.activeElement as HTMLButtonElement);
      const next = event.key === "ArrowDown" ? (index + 1) % buttons.length : event.key === "ArrowUp" ? (index - 1 + buttons.length) % buttons.length : event.key === "Home" ? 0 : event.key === "End" ? buttons.length - 1 : null;
      if (next !== null) { event.preventDefault(); buttons[next]?.focus(); }
      if (event.key === "Tab") setOpen(false);
    }}>
      <header>{t("fork.title")}</header>
      {options.map(option => <button type="button" role="menuitem" key={option.environment} disabled={busy || !!record}
        onClick={() => void fork(option.environment)}>
        <ActionIcon name="fork"/>
        <span><strong>{t(option.label)}</strong><small>{t(option.description)}</small></span>
      </button>)}
      {stopped && <div className="turn-fork-feedback" role="group" aria-label={t("fork.pendingLabel")}>
        <p>{t("fork.notFinished", { target: t(pending.environment === "worktree" ? "fork.newWorktree" : "fork.currentWorkspace") })}</p>
        {(failure || receipt?.error) && <p>{failure || displayBackendText(receipt!.error!)}</p>}
        <button type="button" role="menuitem" disabled={busy} onClick={() => void operate(async () => opened(pending, await forkOperations.resend(client, pending)))}>{t("fork.retry")}</button>
        <button type="button" role="menuitem" disabled={busy} onClick={() => { forkOperations.forget(pending); setError(""); setFailure(""); }}>{t("fork.discard")}</button>
      </div>}
      {running && <div className="turn-fork-hint"><LoadingStatus>{t("fork.creating")}</LoadingStatus></div>}
      {busy && <div className="turn-fork-hint"><LoadingStatus>{t("fork.creating")}</LoadingStatus></div>}
      {error && <p className="turn-fork-feedback" role="alert">{error}</p>}
    </FloatingPopover>}
    {stopped && !open && <span className="turn-fork-feedback">{t("fork.pendingHint")}</span>}
    {(busy || running) && !open && <span className="turn-fork-feedback"><LoadingStatus>{t("fork.creating")}</LoadingStatus></span>}
    {error && !open && <span className="turn-fork-feedback" role="alert">{error}</span>}
  </div>;
}
