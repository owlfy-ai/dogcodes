/** Buffer startup input and coalesce bursts while keeping a single PTY write in flight. */
export function terminalInput(write: (id: string, bytes: number[]) => Promise<void>, failed: (error: unknown) => void) {
  let id: string | undefined, stopped = false, busy = false, scheduled = false;
  let pending: number[] = [];
  const schedule = () => {
    if (stopped || busy || scheduled || !id || !pending.length) return;
    scheduled = true;
    queueMicrotask(() => { scheduled = false; void drain(); });
  };
  const drain = async () => {
    if (stopped || busy || !id) return;
    busy = true;
    try {
      while (!stopped && pending.length) {
        const chunk = pending.splice(0, 8192);
        await write(id, chunk);
      }
    } catch (error) {
      // A write may have partially reached the PTY: never replay or send a
      // truncated remainder of a command after an uncertain failure.
      stopped = true; pending = []; failed(error);
    } finally { busy = false; }
  };
  return {
    push(bytes: Uint8Array) { if (!stopped) { for (const byte of bytes) pending.push(byte); schedule(); } },
    ready(sessionId: string) { if (!stopped) { id = sessionId; schedule(); } },
    dispose() { stopped = true; pending = []; },
  };
}
