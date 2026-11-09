import { expect, it } from "vitest";
import { activityMcpTool, isComputerUseServer, mcpToolLabel, parseMcpTool } from "./mcpToolName";

const cu = "dogcodes_cu_44d31cb7037d";

it("parses each wire spelling of an MCP tool name", () => {
  expect(parseMcpTool(`mcp__${cu}__list_windows`)).toEqual({ server: cu, tool: "list_windows" });
  expect(parseMcpTool("mcp__github__create_issue")).toEqual({ server: "github", tool: "create_issue" });
  expect(parseMcpTool(`@${cu}/click`)).toEqual({ server: cu, tool: "click" });
  expect(parseMcpTool(`${cu}::click`)).toEqual({ server: cu, tool: "click" });
  expect(parseMcpTool(`${cu}__click`)).toEqual({ server: cu, tool: "click" });
});

it("does not treat ordinary names or commands as MCP tools", () => {
  for (const raw of ["Bash", "read_file", "some__thing", `echo mcp__${cu}__click`, "mcp____x", "", undefined]) {
    expect(parseMcpTool(raw)).toBeNull();
  }
});

it("reads the tool from the input only for opaque or wrapper rows", () => {
  const params = { toolName: `mcp__${cu}__click`, params: { pid: 7 } };
  expect(activityMcpTool({ name: "call_9476e867b51744d881f78e2f", input: params })?.tool).toBe("click");
  expect(activityMcpTool({ name: "DeferExecuteTool", input: params })?.tool).toBe("click");
  expect(activityMcpTool({ input: { input: { tool_name: `${cu}__list_windows` } } })?.tool).toBe("list_windows");
  // A real tool keeps its identity whatever its arguments say.
  expect(activityMcpTool({ name: "Bash", input: params })).toBeNull();
  expect(activityMcpTool({ name: "call_9476e867", input: { ...params, command: "ls" } })).toBeNull();
});

it("labels internal servers by tool only and others with their server", () => {
  expect(isComputerUseServer(cu)).toBe(true);
  expect(isComputerUseServer("dogcodes_44d31cb7037d")).toBe(false);
  expect(mcpToolLabel({ server: "dogcodes_44d31cb7037d", tool: "getTurns" })).toBe("getTurns");
  expect(mcpToolLabel({ server: "github", tool: "create_issue" })).toBe("github · create_issue");
});
