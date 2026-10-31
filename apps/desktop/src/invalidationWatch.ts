import { Channel, invoke } from "@tauri-apps/api/core";
import type { WorkspaceWatchMessage } from "./workspaceWatch";
import { translate as t } from "./i18n";

type Listener = { refresh: () => void; error?: (message: string) => void };
/** Shared reconnect/cleanup mechanics for read-only service invalidations. */
export function createInvalidationSubscriber(watchCommand: string, unwatchCommand: string) {
  const subscriptions = new Map<string, { listeners: Set<Listener>; stop: () => void }>();
  return function subscribe(key: string, args: Record<string, unknown>, refresh: () => void, error?: (message: string) => void): () => void {
    const listener = { refresh, error };
    let entry = subscriptions.get(key);
    if (!entry) {
      const listeners = new Set<Listener>();
      let stopped = false, epoch = 0, failures = 0, id: string | undefined;
      let timer: ReturnType<typeof setTimeout> | undefined;
      const unwatch = (id: string) => { void invoke(unwatchCommand, { id }).catch(() => {}); };
      const connect = () => {
        if (stopped) return;
        const attempt = ++epoch, subscription = crypto.randomUUID(); id = subscription;
        let ended = false;
        const active = () => !stopped && attempt === epoch && !ended;
        const fail = (message: string) => {
          if (!active()) return;
          ended = true; unwatch(subscription);
          listeners.forEach(item => { item.error?.(t("watch.syncLost", { message })); item.refresh(); });
          timer = setTimeout(connect, Math.min(15000, 1000 * 2 ** Math.min(failures++, 4)));
        };
        try {
          const changes = new Channel<WorkspaceWatchMessage>();
          changes.onmessage = event => {
            if (!active()) return;
            if (event.kind === "invalidated") listeners.forEach(item => item.refresh());
            else if (event.kind === "error") fail(event.message);
          };
          void invoke(watchCommand, { ...args, id: subscription, changes }).then(() => {
            if (!active()) { unwatch(subscription); return; }
            failures = 0; listeners.forEach(item => { item.error?.(""); item.refresh(); });
          }).catch(cause => fail(String(cause)));
        } catch (cause) {
          // Channel registration may fail synchronously before invoke can return
          // a promise. Keep the view mounted and use the same bounded recovery.
          fail(String(cause));
        }
      };
      entry = { listeners, stop: () => { stopped = true; epoch++; clearTimeout(timer); if (id) unwatch(id); } };
      subscriptions.set(key, entry);
      // Add the first listener before even a synchronously resolved transport responds.
      listeners.add(listener); connect();
    } else entry.listeners.add(listener);
    const current = entry;
    return () => {
      current.listeners.delete(listener);
      if (!current.listeners.size && subscriptions.get(key) === current) { subscriptions.delete(key); current.stop(); }
    };
  }
}
