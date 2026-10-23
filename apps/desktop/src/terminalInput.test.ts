import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { terminalInput } from "./terminalInput";
import { inputFixture, inputTick, testCrypto } from "./terminalInput.fixture";
const encode = (text: string) => new TextEncoder().encode(text);
beforeEach(() => vi.stubGlobal("crypto", testCrypto));
afterEach(() => { vi.unstubAllGlobals(); vi.useRealTimers(); });
it("buffers startup, Unicode and control keys with one write in flight", async () => {
  const f = inputFixture(), api = f.client(); let finish!: () => void;
  f.gate = new Promise<void>(resolve => { finish = resolve; });
  const queue = terminalInput(api, vi.fn()); queue.push(encode("echo "));
  await inputTick(); expect(api.get).not.toHaveBeenCalled();
  queue.ready("pty"); await inputTick(); expect(f.writes.length).toBe(1);
  for (const letter of "中文abcdef\r\x03") queue.push(encode(letter));
  await inputTick(); expect(f.writes.length).toBe(1);
  finish(); await inputTick();
  expect(f.writes.length).toBe(2);
  expect(new TextDecoder().decode(Uint8Array.from(f.writes.flat()))).toBe("echo 中文abcdef\r\x03"); queue.dispose();
});
it("coalesces bursts and splits long pastes without losing bytes", async () => {
  const f = inputFixture(), queue = terminalInput(f.client(), vi.fn()); queue.ready("pty");
  const text = "中文abc".repeat(6000);
  for (const letter of text) queue.push(encode(letter));
  await inputTick(); expect(f.writes.length).toBeGreaterThan(1); expect(f.writes.length).toBeLessThan(10);
  expect(f.writes.every(bytes => bytes.length <= 8192)).toBe(true);
  expect(new TextDecoder().decode(Uint8Array.from(f.writes.flat()))).toBe(text); queue.dispose();
});
it("a lost applied reply is proved by read-only inspect, never replayed", async () => {
  const f = inputFixture(); f.loseReply = true; const api = f.client(), queue = terminalInput(api, vi.fn());
  queue.ready("pty"); queue.push(encode("once\n")); await inputTick();
  expect(queue.state.paused).toBe(false); expect(f.writes.length).toBe(1);
  expect(vi.mocked(api.input).mock.calls.filter(call => call[1].type === "write").length).toBe(1);
  expect(vi.mocked(api.input).mock.calls.at(-1)?.[1].type).toBe("inspect"); queue.dispose();
});
it("unadmitted input stays paused until explicit new CAS; no original bytes are replayed", async () => {
  const f = inputFixture(); f.notAdmitted = true; const api = f.client(), queue = terminalInput(api, vi.fn());
  queue.ready("pty"); queue.push(encode("old\n")); await inputTick(); expect(queue.state.hasUnknown).toBe(true);
  queue.push(encode("discard\n")); await inputTick();
  const before = vi.mocked(api.input).mock.calls.length;
  expect(await queue.verify()).toBe(false);
  expect(vi.mocked(api.input).mock.calls.slice(before).map(call => call[1].type)).toEqual(["inspect"]);
  f.notAdmitted = false; expect(await queue.resume()).toBe(true);
  expect(f.writes).toEqual([]); queue.push(encode("new\n")); await inputTick();
  expect(new TextDecoder().decode(Uint8Array.from(f.writes.flat()))).toBe("new\n"); queue.dispose();
});
it("another window or phone invalidates old input and stale writes cannot be adopted", async () => {
  const f = inputFixture(), a = terminalInput(f.client("main"), vi.fn()), b = terminalInput(f.client("phone"), vi.fn());
  a.ready("pty"); a.push(encode("A")); await inputTick();
  b.ready("pty"); b.push(encode("B")); await inputTick();
  a.push(encode("stale")); await inputTick(); expect(a.state.paused).toBe(true); expect(await a.verify()).toBe(false);
  expect(new TextDecoder().decode(Uint8Array.from(f.writes.flat()))).toBe("AB");
  expect(await a.resume()).toBe(true); a.push(encode("C")); await inputTick();
  expect(new TextDecoder().decode(Uint8Array.from(f.writes.flat()))).toBe("ABC"); a.dispose(); b.dispose();
});
it("output reconnect preserves pending proof and does not automatically resume input", async () => {
  const f = inputFixture(); let finish!: () => void; f.gate = new Promise<void>(resolve => { finish = resolve; });
  const api = f.client(), queue = terminalInput(api, vi.fn()); queue.ready("pty"); queue.push(encode("one")); await inputTick();
  queue.push(encode("discard")); queue.suspend(); finish(); await inputTick(); queue.ready("pty");
  expect(queue.state.paused).toBe(true); expect(queue.state.hasUnknown).toBe(true); expect(f.writes.length).toBe(1);
  expect(await queue.verify()).toBe(true); expect(queue.state.paused).toBe(false); expect(f.writes.length).toBe(1); queue.dispose();
});
it("dispose or suspend during preparation cannot steal a new input lease", async () => {
  for (const stop of ["dispose", "suspend"] as const) {
    const f = inputFixture(), api = f.client(); let finish!: () => void;
    vi.mocked(api.get).mockImplementation(async id => { await new Promise<void>(resolve => { finish = resolve; }); return { id, epoch: "epoch", status: "running" }; });
    const queue = terminalInput(api, vi.fn()); queue.ready("pty"); queue.push(encode("old")); await inputTick();
    queue[stop](); finish(); await inputTick(); expect(api.input).not.toHaveBeenCalled(); queue.dispose();
  }
});
it("queue overflow discards unsubmitted bytes, and a changed backend is never acquired", async () => {
  const f = inputFixture(), api = f.client(), queue = terminalInput(api, vi.fn()); queue.ready("pty");
  queue.push(new Uint8Array(65537)); await inputTick(); expect(api.input).not.toHaveBeenCalled(); expect(queue.state.paused).toBe(true);
  expect(await queue.resume()).toBe(true); queue.suspend(); f.current("pty").epoch = "replacement";
  expect(await queue.resume()).toBe(false); expect(queue.state.canResume).toBe(false);
  expect(f.current("pty").revision).toBe("1"); queue.dispose();
});

it("a malformed applied receipt is not proof; verifying corrected metadata never repeats bytes", async () => {
  const f = inputFixture(), api = f.client(), native = api.input; let corrupt = true;
  api.input = vi.fn(async (id, request) => {
    const reply = await native(id, request) as Record<string, unknown>;
    return corrupt && reply.state === "applied" ? { ...reply, written: 0 } : reply;
  });
  const queue = terminalInput(api, vi.fn()); queue.ready("pty"); queue.push(encode("once")); await inputTick();
  expect(queue.state.paused).toBe(true); expect(await queue.verify()).toBe(false);
  corrupt = false; expect(await queue.verify()).toBe(true); expect(f.writes.length).toBe(1); queue.dispose();
});
