import { createHash, randomUUID } from "node:crypto";
import { vi } from "vitest";
import type { TerminalInputApi } from "./terminalInput";
export const testCrypto = { randomUUID, subtle: { digest: async (_algorithm: string, data: Uint8Array) => Uint8Array.from(createHash("sha256").update(data).digest()).buffer } };
export const inputTick = async () => { for (let i = 0; i < 150; i++) await Promise.resolve(); };
/** Models the PC's shared CAS lease/receipt, independently of each view. */
export function inputFixture() {
  const states = new Map<string, Record<string, unknown>>(), writes: number[][] = [];
  let gate: Promise<void> | undefined;
  let loseReply = false, notAdmitted = false, unavailable = false;
  const current = (id: string) => {
    let value = states.get(id);
    if (!value) { value = { terminalId: id, epoch: "epoch", revision: "0", streamId: null, owner: null, sequence: "0", state: "idle", uncertain: false, digest: null, written: 0, status: "running" }; states.set(id, value); }
    return value;
  };
  const client = (owner = "main"): TerminalInputApi => ({
    get: vi.fn(async id => ({ id, epoch: String(current(id).epoch), status: String(current(id).status) })),
    input: vi.fn(async (id, request) => {
      if (unavailable) throw new Error("connection lost");
      const value = current(id), snapshot = () => ({ ...value, owner: undefined, owned: value.owner === owner });
      if (request.terminalId !== id || request.expectedEpoch !== value.epoch) throw new Error("epoch changed");
      if (request.type === "inspect") return snapshot();
      if (request.type === "open") {
        if (request.expectedRevision !== value.revision) throw new Error("CAS conflict");
        Object.assign(value, { owner, streamId: request.streamId, revision: (BigInt(value.revision as string) + 1n).toString(), sequence: "0", state: "ready", digest: null, written: 0 });
        if (request.acknowledgeUncertain) value.uncertain = false;
        return snapshot();
      }
      if (request.type !== "write" || request.streamId !== value.streamId || request.revision !== value.revision || value.owner !== owner || BigInt(request.sequence as string) !== BigInt(value.sequence as string) + 1n) throw new Error("stale input");
      if (notAdmitted) throw new Error("reply lost before admission");
      const bytes = [...Buffer.from(request.data as string, "base64")]; writes.push(bytes);
      Object.assign(value, { sequence: request.sequence, digest: createHash("sha256").update(Uint8Array.from(bytes)).digest("hex"), state: "writing" });
      await gate;
      value.state = "applied"; value.written = bytes.length;
      if (loseReply) throw new Error("reply lost after applying");
      return snapshot();
    }),
  });
  return { current, writes, client, set gate(value: Promise<void> | undefined) { gate = value; }, set loseReply(value: boolean) { loseReply = value; }, set notAdmitted(value: boolean) { notAdmitted = value; }, set unavailable(value: boolean) { unavailable = value; } };
}
