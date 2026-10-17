import { expect, it } from "vitest";
import { terminalCommand, terminalOutput, truncateTerminalOutput } from "./terminalPresentation";

it("extracts commands and output without protocol metadata", () => {
  expect(terminalCommand({ command: "pwd", timeout: 1000 })).toBe("pwd");
  expect(terminalCommand({ cmd: ["sh", "-c", "echo 'hello'"] })).toBe("sh -c 'echo '\\''hello'\\''' ".trim());
  expect(terminalOutput({ stdout: "hello", stderr: "warning", exit_code: 1 })).toBe("hello\nwarning");
  expect(terminalOutput([{ type: "text", text: "result" }])).toBe("result");
  expect(terminalOutput({ exit_code: 0 })).toBe("");
  expect(terminalOutput('{"actual_command_output":true}')).toBe('{"actual_command_output":true}');
});

it("bounds both multiline logs and single long lines", () => {
  expect(truncateTerminalOutput("ok")).toEqual({ text: "ok", truncated: false });
  const lines = truncateTerminalOutput(Array.from({ length: 20 }, (_, i) => String(i)).join("\n"));
  expect(lines.text.split("\n")).toHaveLength(12);
  expect(lines.truncated).toBe(true);
  expect(truncateTerminalOutput("x".repeat(4000))).toEqual({ text: "x".repeat(3000), truncated: true });
});
