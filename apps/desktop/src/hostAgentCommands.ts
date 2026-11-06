/** Which agents a server can run, and what its shell runs to sign in to one. */

/** Packaged as a desktop application, with no CLI of its own to install. */
const NOT_ON_SERVERS = new Set(["zcode"]);
export const runsOnServers = (providerId: string) => !NOT_ON_SERVERS.has(providerId);

const LOGIN: Record<string, string[]> = {
  codex: ["login"], cursor: ["login"], grok: ["login"], kimi: ["login"], kiro: ["login"],
  "claude-code": ["auth", "login"], opencode: ["auth", "login"],
};
const quoted = (value: string) => /^[\w./:=@%+-]+$/.test(value) ? value : `'${value.replaceAll("'", "'\\''")}'`;
/** Agents without a login command sign in from their own interface, so that's what starts. */
export const loginCommand = (providerId: string, executable: string) => [quoted(executable), ...(LOGIN[providerId] ?? [])].join(" ");
