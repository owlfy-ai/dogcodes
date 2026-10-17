import rules from "./workspaceNoise.json";
const directories = new Set(rules.directories);
const files = new Set(rules.files);
export function isWorkspaceNoise(path: string) {
  const parts = path.replaceAll("\\", "/").toLowerCase().split("/");
  return parts.some(name => name.startsWith(".") || directories.has(name) || files.has(name) || rules.suffixes.some(suffix => name.endsWith(suffix)));
}
