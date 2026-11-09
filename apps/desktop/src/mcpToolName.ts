// Agents report MCP tools by their wire names (`mcp__server__tool`,
// `@server/tool`, `server::tool`, or an opaque call id with the name in its
// input). Those names are protocol detail, not something to show a person.

export type McpTool = { server: string; tool: string };

// Servers dogcodes injects itself: history (`dogcodes_<12 hex>`) and
// computer use (`dogcodes_cu_<12 hex>`); see crates/core.
const INTERNAL_SERVER = /^dogcodes_(?:cu_)?[0-9a-f]{12}$/;
const COMPUTER_SERVER = /^dogcodes_cu_[0-9a-f]{12}$/;
const OPAQUE_CALL = /^(?:call_|chatcmpl-tool-)[0-9a-f]{8,128}$/i;
// Native names some agents give the call that wraps a deferred MCP tool.
const WRAPPERS = new Set(["defer_execute_tool", "deferexecutetool", "usetool", "use_tool", "mcp", "mcp_tool"]);

export function parseMcpTool(raw: unknown): McpTool | null {
  if (typeof raw !== "string" || !raw || /\s/.test(raw)) return null;
  const match =
    /^mcp__(.+?)__(.+)$/.exec(raw) ??
    /^@([^/]+)\/([^/]+)$/.exec(raw) ??
    /^([^:]+)::([^:]+)$/.exec(raw) ??
    // Unqualified `server__tool` only for servers we know we injected.
    /^(dogcodes_(?:cu_)?[0-9a-f]{12})__(.+)$/.exec(raw);
  return match ? { server: match[1], tool: match[2] } : null;
}

function wrappedName(input: unknown): unknown {
  let value = input;
  for (let depth = 0; depth < 3 && value && typeof value === "object"; depth++) {
    const record = value as Record<string, unknown>;
    // A command row that mentions a tool name is still a command.
    if (record.command != null) return undefined;
    const name = record.toolName ?? record.tool_name ?? record.tool_id;
    if (name != null) return name;
    value = record.rawInput ?? record.raw_input ?? record.input;
  }
  return undefined;
}

export function activityMcpTool(a: { name?: string; title?: string; input?: unknown }): McpTool | null {
  const direct = parseMcpTool(a.name) ?? parseMcpTool(a.title);
  if (direct) return direct;
  const name = (a.name ?? "").trim();
  // Only trust a name carried in the input when the row itself has no real
  // tool name: never relabel a known tool because of its arguments.
  if (name && !OPAQUE_CALL.test(name) && !WRAPPERS.has(name.toLowerCase())) return null;
  return parseMcpTool(wrappedName(a.input));
}

export const isComputerUseServer = (server: string) => COMPUTER_SERVER.test(server);

/** Short label: internal servers show only the tool, others `server · tool`. */
export function mcpToolLabel({ server, tool }: McpTool): string {
  return INTERNAL_SERVER.test(server) ? tool : `${server} · ${tool}`;
}
