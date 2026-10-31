import { userFacingError } from "./userFacingError";
import { translate as t } from "./i18n";

/** Ephemeral, fenced input for one terminal view. No keystrokes are persisted. */
export interface TerminalInputApi {
  get(id: string): Promise<{ id: string; epoch: string; status: string }>;
  input(id: string, request: Record<string, unknown>): Promise<unknown>;
}
export interface TerminalInputState {
  paused: boolean; busy: boolean; hasUnknown: boolean; canResume: boolean; message: string | null;
}
type Receipt = Record<string, unknown> & { epoch: string; revision: string; sequence: string };
const sequence = (value: unknown): bigint => {
  if (typeof value !== "string" || !/^\d{1,20}$/.test(value) || BigInt(value) > 18446744073709551615n) throw new Error(t("terminalInput.invalidReceipt"));
  return BigInt(value);
};
async function timeout<T>(value: Promise<T>, ms: number): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try { return await Promise.race([value, new Promise<never>((_, reject) => { timer = setTimeout(() => reject(new Error(t("terminalInput.timeout"))), ms); })]); }
  finally { clearTimeout(timer); }
}
export function terminalInput(api: TerminalInputApi, changed: (state: TerminalInputState) => void) {
  let id: string | undefined, epoch: string | undefined, lease: Receipt | undefined, unknown: Receipt | undefined;
  let stopped = false, busy = false, paused = false, scheduled = false, canResume = true, fence = 0, inflight = 0;
  let pending: number[] = [], last = "0", message: string | null = null;
  const state = (): TerminalInputState => ({ paused, busy, hasUnknown: !!unknown, canResume, message });
  const emit = () => { if (!stopped) changed(state()); };
  const check = (ticket: number) => { if (stopped || fence !== ticket) throw new Error(t("terminalInput.paused")); };
  const pause = (text: string) => { ++fence; paused = true; pending = []; message = text; emit(); };
  const call = async (request: Record<string, unknown>, ticket: number): Promise<Receipt> => {
    check(ticket);
    const value = await timeout(api.input(id!, { ...request, terminalId: id }), request.type === "write" ? 8000 : 3000);
    check(ticket);
    if (!value || typeof value !== "object") throw new Error(t("terminalInput.receiptMismatch"));
    const result = value as Receipt;
    if (result.terminalId !== id || typeof result.epoch !== "string" || typeof result.owned !== "boolean") throw new Error(t("terminalInput.receiptMismatch"));
    sequence(result.revision); sequence(result.sequence);
    return result;
  };
  const inspect = (expected: Receipt, ticket: number) => call({ type: "inspect", expectedEpoch: expected.epoch }, ticket);
  const applied = (reply: Receipt, expected: Receipt) => ["epoch", "revision", "streamId", "sequence", "digest"].every(key => reply[key] === expected[key])
    && reply.owned === true && reply.state === "applied" && reply.uncertain === false && reply.written === expected.byteLength;
  const open = async (acknowledge: boolean, ticket: number) => {
    check(ticket);
    const info = await timeout(api.get(id!), 3000); check(ticket);
    if (info.id !== id || typeof info.epoch !== "string" || (epoch && epoch !== info.epoch)) {
      canResume = false; throw new Error(t("terminalInput.backendChangedReopen"));
    }
    if (info.status !== "running") {
      canResume = info.status === "starting";
      throw new Error(t(canResume ? "terminalInput.starting" : "terminalInput.notWritable"));
    }
    epoch = info.epoch;
    const original = await call({ type: "inspect", expectedEpoch: epoch }, ticket);
    if (original.epoch !== epoch) throw new Error(t("terminalInput.backendChanged"));
    const stream = crypto.randomUUID();
    const opened = await call({ type: "open", expectedEpoch: epoch, expectedRevision: original.revision, streamId: stream, acknowledgeUncertain: acknowledge }, ticket);
    if (opened.streamId !== stream || opened.epoch !== epoch || opened.owned !== true || sequence(opened.revision) !== sequence(original.revision) + 1n) throw new Error(t("terminalInput.connectionChanged"));
    lease = opened; last = opened.sequence;
    if (opened.state !== "ready" || opened.uncertain !== false) throw new Error(t(opened.state === "opening" ? "terminalInput.switching" : "terminalInput.partial"));
  };
  const run = async () => {
    if (busy || stopped || paused || !id) return;
    busy = true; emit();
    try {
      while (!stopped && !paused && pending.length) {
        const chunk = pending.splice(0, 8192), ticket = fence;
        inflight = chunk.length;
        try {
          if (!lease) await open(false, ticket);
          check(ticket);
          const digest = [...new Uint8Array(await crypto.subtle.digest("SHA-256", new Uint8Array(chunk)))].map(byte => byte.toString(16).padStart(2, "0")).join("");
          check(ticket);
          const next = (sequence(last) + 1n).toString();
          const expected: Receipt = { epoch: lease!.epoch, revision: lease!.revision, sequence: next, streamId: lease!.streamId, digest, byteLength: chunk.length };
          unknown = expected;
          let receipt: Receipt;
          try { receipt = await call({ type: "write", expectedEpoch: expected.epoch, streamId: expected.streamId, revision: expected.revision, sequence: next, data: btoa(String.fromCharCode(...chunk)) }, ticket); }
          catch { receipt = await inspect(expected, ticket); }
          if (!applied(receipt, expected)) throw new Error(t("terminalInput.unconfirmed"));
          last = next; unknown = undefined; message = null;
        } catch (error) {
          if (!stopped && fence === ticket) pause(userFacingError(error, t("terminalInput.resultUnverified")));
        } finally { inflight = 0; }
      }
    } finally { busy = false; emit(); }
  };
  const schedule = () => {
    if (stopped || busy || scheduled || paused || !id || !pending.length) return;
    scheduled = true;
    queueMicrotask(() => { scheduled = false; void run(); });
  };
  return {
    get state() { return state(); },
    get attached() { return !!lease && !paused && !stopped && canResume; },
    push(bytes: Uint8Array) {
      if (stopped || paused || !canResume || !bytes.length) return;
      if (pending.length + inflight + bytes.length > 65536) { pause(t("terminalInput.tooMuch")); return; }
      for (const byte of bytes) pending.push(byte);
      schedule();
    },
    ready(sessionId: string) {
      if (stopped) return;
      if (id && id !== sessionId) { canResume = false; pause(t("terminalInput.idChanged")); return; }
      id = sessionId; schedule();
    },
    suspend() { if (!stopped) pause(t("terminalInput.disconnected")); },
    ended() {
      if (stopped) return;
      canResume = false; ++fence; pending = [];
      if (unknown) { paused = true; message = t("terminalInput.ended"); }
      emit();
    },
    async verify() {
      if (stopped || busy || !unknown) return false;
      const expected = unknown, ticket = fence; busy = true; emit();
      try {
        const reply = await inspect(expected, ticket);
        if (applied(reply, expected)) { last = expected.sequence; unknown = undefined; paused = false; message = null; return true; }
        message = t("terminalInput.takenOver");
      } catch { if (!stopped && fence === ticket) message = t("terminalInput.verifyFailed"); }
      finally { busy = false; emit(); }
      return false;
    },
    async resume() {
      if (stopped || busy || !canResume || !id) return false;
      const ticket = fence; busy = true; lease = undefined; emit();
      try { await open(true, ticket); check(ticket); unknown = undefined; paused = false; message = null; return true; }
      catch (error) { if (!stopped && fence === ticket) { paused = true; message = userFacingError(error, t("terminalInput.notRecovered")); } }
      finally { busy = false; emit(); }
      return false;
    },
    dispose() { stopped = true; ++fence; pending = []; },
  };
}
