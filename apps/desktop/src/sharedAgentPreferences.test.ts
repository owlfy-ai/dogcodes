import { afterEach, expect, it, vi } from "vitest";
import type { AgentPreferences, AgentPreferencesSnapshot, EventStreamMessage } from "@dogcodes/protocol";
import { SharedAgentPreferences } from "./sharedAgentPreferences";

const empty = (): AgentPreferences => ({ order: [], disabled: [] });
const stores: SharedAgentPreferences[] = [];
afterEach(() => { for (const store of stores.splice(0)) store.stop(); vi.useRealTimers(); });
async function flush() { for (let i = 0; i < 12; i++) await Promise.resolve(); }
function host(initial: AgentPreferencesSnapshot = { revision: 0, lastSequence: 40, preferences: empty() }) {
  let current = structuredClone(initial);
  const listeners = new Set<(message: EventStreamMessage) => void>();
  const events: EventStreamMessage[] = [];
  const commit = (preferences: AgentPreferences) => {
    current = { revision: current.revision + 1, lastSequence: current.lastSequence + 1, preferences: structuredClone(preferences) };
    const message: EventStreamMessage = { type: "event", data: {
      schemaVersion: 1, eventId: "event", sequence: current.lastSequence, streamVersion: current.revision,
      stream: { streamType: "system", streamId: "preferences.agents" }, eventType: "preferences.agents.updated",
      payload: structuredClone(preferences), source: { kind: "hub" }, occurredAt: "now", recordedAt: "now",
      correlationId: "correlation", causationId: null, vendorExtensions: null,
    } };
    events.push(message);
    for (const listener of listeners) listener(message);
  };
  const client = {
    agentPreferences: vi.fn(async () => structuredClone(current)),
    setAgentPreferences: vi.fn(async (preferences: AgentPreferences, revision: number, _operationId: string) => {
      if (revision !== current.revision) throw new Error("conflict");
      commit(preferences);
    }),
    subscribe: vi.fn(async (after: number, listener: (message: EventStreamMessage) => void) => {
      listeners.add(listener);
      for (const event of events) if (event.type === "event" && event.data.sequence > after) listener(event);
      return () => { listeners.delete(listener); };
    }),
  };
  const open = async (legacy = empty()) => {
    const store = new SharedAgentPreferences(client, legacy); stores.push(store); store.start(); await flush(); return store;
  };
  return { client, commit, open, current: () => current, listeners };
}

it("uses the host value across two clients and reopening, retaining local settings only as backup", async () => {
  const fixture = host({ revision: 3, lastSequence: 50, preferences: { order: ["kimi", "codex"], disabled: ["codex"] } });
  const a = await fixture.open({ order: ["cursor"], disabled: ["kimi"] });
  const b = await fixture.open();
  expect(a.getSnapshot().importRequired).toBe(false);
  expect(a.getSnapshot().snapshot?.preferences).toEqual(fixture.current().preferences);
  await a.write({ order: ["codex", "kimi"], disabled: [] });
  expect(b.getSnapshot().snapshot?.preferences.disabled).toEqual([]);
  a.stop();
  const reopened = await fixture.open({ order: ["cursor"], disabled: ["kimi"] });
  expect(reopened.getSnapshot().snapshot?.revision).toBe(4);
  expect(fixture.client.setAgentPreferences).toHaveBeenCalledTimes(1);
});

it("requires an explicit import/default choice and never overwrites another client's winning initialization", async () => {
  const fixture = host();
  const legacy = { order: ["kimi"], disabled: ["codex"] };
  const first = await fixture.open(legacy);
  const second = await fixture.open({ order: ["cursor"], disabled: [] });
  expect(first.getSnapshot().importRequired).toBe(true);
  expect(fixture.client.setAgentPreferences).not.toHaveBeenCalled();
  await first.importLegacy(true);
  await second.importLegacy(true);
  expect(fixture.current().preferences).toEqual(legacy);
  expect(fixture.client.setAgentPreferences).toHaveBeenCalledTimes(1);
  expect(second.getSnapshot().importRequired).toBe(false);
  const fresh = host();
  const defaults = await fresh.open(legacy);
  await defaults.importLegacy(false);
  expect(fresh.current()).toMatchObject({ revision: 1, preferences: empty() });
});

it("waits for durable state rather than presenting an unacknowledged write as saved", async () => {
  const fixture = host();
  const store = await fixture.open();
  let resolve!: () => void;
  fixture.client.setAgentPreferences.mockImplementationOnce(() => new Promise<void>(yes => { resolve = yes; }));
  const desired = { order: ["kimi"], disabled: ["codex"] };
  const saving = store.write(desired);
  expect(store.getSnapshot().busy).toBe(true);
  expect(store.getSnapshot().snapshot?.preferences).toEqual(empty());
  await store.write(empty());
  expect(fixture.client.setAgentPreferences).toHaveBeenCalledTimes(1);
  fixture.commit(desired); resolve(); await saving;
  expect(store.getSnapshot().snapshot?.preferences).toEqual(desired);
  expect(store.getSnapshot().busy).toBe(false);
});

it("reconciles a lost ack from current state without issuing a second mutation", async () => {
  const fixture = host();
  fixture.client.setAgentPreferences.mockImplementationOnce(async preferences => { fixture.commit(preferences); throw new Error("lost reply"); });
  const store = await fixture.open();
  await store.write({ order: ["kimi"], disabled: [] });
  expect(store.getSnapshot().snapshot?.revision).toBe(1);
  expect(store.getSnapshot().unconfirmed).toBe(false);
  expect(store.getSnapshot().error).toContain("后台当前设置");
  await store.retry();
  expect(fixture.client.setAgentPreferences).toHaveBeenCalledTimes(1);
});

it("retries an unknown result with the same request and prevents intervening edits", async () => {
  const fixture = host();
  fixture.client.setAgentPreferences.mockRejectedValueOnce(new Error("not delivered"));
  const store = await fixture.open();
  await store.write({ order: ["kimi"], disabled: [] });
  const original = fixture.client.setAgentPreferences.mock.calls[0];
  expect(store.getSnapshot().unconfirmed).toBe(true);
  await store.write({ order: ["cursor"], disabled: [] });
  expect(fixture.client.setAgentPreferences).toHaveBeenCalledTimes(1);
  await store.retry();
  expect(fixture.client.setAgentPreferences.mock.calls[1]).toEqual(original);
  expect(store.getSnapshot().snapshot?.preferences.order).toEqual(["kimi"]);
  expect(store.getSnapshot().unconfirmed).toBe(false);
});

it("does not let a delayed ack replace a newer edit from another client", async () => {
  const fixture = host();
  const store = await fixture.open();
  let resolve!: () => void;
  fixture.client.setAgentPreferences.mockImplementationOnce(preferences => {
    fixture.commit(preferences);
    return new Promise<void>(yes => { resolve = yes; });
  });
  const pending = store.write({ order: ["kimi"], disabled: [] });
  fixture.commit({ order: ["codex"], disabled: ["kimi"] });
  resolve(); await pending;
  expect(store.getSnapshot().snapshot?.revision).toBe(2);
  expect(store.getSnapshot().snapshot?.preferences.disabled).toEqual(["kimi"]);
});

it("replays changes between snapshot and subscribe, then reloads on a stream gap", async () => {
  vi.useFakeTimers();
  const fixture = host();
  fixture.client.agentPreferences.mockImplementationOnce(async () => {
    const before = structuredClone(fixture.current());
    fixture.commit({ order: ["kimi"], disabled: [] });
    return before;
  });
  const store = await fixture.open();
  expect(store.getSnapshot().snapshot?.revision).toBe(1);
  // A lost stream frame cannot silently leave settings stale.
  for (const listener of fixture.listeners) listener({ type: "event", data: { sequence: 100 } } as EventStreamMessage);
  expect(store.getSnapshot().error).toContain("重新连接");
  await vi.advanceTimersByTimeAsync(500);
  expect(fixture.client.agentPreferences).toHaveBeenCalledTimes(2);
  expect(store.getSnapshot().error).toBe("");
  store.stop();
  expect(fixture.listeners.size).toBe(0);
});

it("ignores an obsolete startup reply after cleanup and reconnect", async () => {
  const fixture = host();
  let resolve!: (value: AgentPreferencesSnapshot) => void;
  fixture.client.agentPreferences.mockImplementationOnce(() => new Promise(yes => { resolve = yes; }));
  const store = await fixture.open();
  store.stop();
  fixture.commit({ order: ["kimi"], disabled: [] });
  store.start(); await flush();
  resolve({ revision: 0, lastSequence: 40, preferences: empty() }); await flush();
  expect(store.getSnapshot().snapshot?.revision).toBe(1);
  expect(fixture.listeners.size).toBe(1);
});
