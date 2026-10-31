import type { AgentPreferences, CoreClient } from "@dogcodes/protocol";
import { SharedPreferences, type SharedPreferencesView } from "./sharedPreferences";

type Client = Required<Pick<CoreClient, "agentPreferences" | "setAgentPreferences" | "subscribe">>;
export type SharedAgentPreferencesView = SharedPreferencesView<AgentPreferences>;
export class SharedAgentPreferences extends SharedPreferences<AgentPreferences> {
  constructor(client: Client, legacy: AgentPreferences) {
    super({ read: () => client.agentPreferences(), write: (value, revision, id) => client.setAgentPreferences(value, revision, id), subscribe: (after, listener) => client.subscribe(after, listener) }, legacy, {
      streamId: "preferences.agents", eventType: "preferences.agents.updated", label: "sharedPrefs.agentsLabel",
      defaults: () => ({ order: [], disabled: [] }), hasLegacy: value => value.order.length > 0 || value.disabled.length > 0,
      valid: value => [value.order, value.disabled].every(ids => ids.length <= 256 && new Set(ids).size === ids.length && ids.every(id => /^[a-zA-Z0-9_.-]{1,128}$/.test(id))),
    });
  }
}
