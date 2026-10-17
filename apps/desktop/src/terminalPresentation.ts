/** Extract display content without exposing native tool envelopes. */
export function terminalCommand(value: unknown): string {
  if (typeof value === "string") return value;
  if (!value || typeof value !== "object") return "";
  const record = value as Record<string, unknown>;
  for (const key of ["command", "cmd", "command_line", "commandLine", "CommandLine", "argv"]) {
    const command = record[key];
    if (typeof command === "string") return command;
    if (Array.isArray(command) && command.every(arg => typeof arg === "string"))
      return command.map(arg => /^[\w./:=@%-]+$/.test(arg) ? arg : `'${arg.replaceAll("'", "'\\''")}'`).join(" ");
  }
  return "";
}

export function terminalOutput(value: unknown): string {
  if (typeof value === "string") return value;
  if (Array.isArray(value)) return value.map(terminalOutput).filter(Boolean).join("\n");
  if (!value || typeof value !== "object") return "";
  const record = value as Record<string, unknown>;
  if (record.stdout != null || record.stderr != null)
    return [record.stdout, record.stderr].map(terminalOutput).filter(Boolean).join("\n");
  for (const key of ["text", "content", "output", "aggregated_output", "aggregatedOutput", "result"]) {
    if (record[key] != null) return terminalOutput(record[key]);
  }
  return "";
}

export function truncateTerminalOutput(text: string): { text: string; truncated: boolean } {
  const preview = text.split("\n").slice(0, 12).join("\n").slice(0, 3000);
  return { text: preview, truncated: preview.length < text.length };
}
