import { useEffect, useState } from "react";
import type { ArtifactMetadata, CoreClient, TaskId } from "@dogcodes/protocol";
import { cachedTurnArtifact, loadTurnArtifact, turnArtifactKey, type TurnArtifact } from "./turnArtifactCache";
import { parseGitDiff } from "./diffView";
import { reportedPatch } from "./conversationDiff";

interface EditSource {
  taskId: TaskId;
  artifact?: ArtifactMetadata;
  files: { path: string; diff?: string }[];
}

export function useToolRunEdits(sources: EditSource[], client: CoreClient) {
  const [loaded, setLoaded] = useState<{ client: CoreClient; values: Map<string, TurnArtifact> }>();
  const artifacts = sources.flatMap(source => source.artifact ? [{ taskId: source.taskId, metadata: source.artifact }] : []);
  const key = JSON.stringify(artifacts);
  useEffect(() => {
    let active = true;
    const entries = JSON.parse(key) as typeof artifacts;
    const unique = new Map(entries.map(entry => [turnArtifactKey(entry.taskId, entry.metadata), entry]));
    if (unique.size) void Promise.all([...unique].map(async ([id, entry]) => {
      try { return [id, await loadTurnArtifact(client, entry.taskId, entry.metadata)] as const; }
      catch { return null; } // The expanded artifact row retains its own error/retry UI.
    })).then(values => {
      if (active) setLoaded({ client, values: new Map(values.filter(value => value !== null)) });
    });
    return () => { active = false; };
  }, [client, key]);

  const files = new Set<string>(), seenArtifacts = new Set<string>();
  let additions = 0, deletions = 0, hasDiff = false, partial = false;
  for (const source of sources) {
    if (source.artifact) {
      const id = turnArtifactKey(source.taskId, source.artifact);
      if (seenArtifacts.has(id)) continue;
      seenArtifacts.add(id);
      const value = cachedTurnArtifact(client, source.taskId, source.artifact)
        ?? (loaded?.client === client ? loaded.values.get(id) : undefined);
      // Captured artifacts are authoritative; do not also count their reported patch.
      if (value) {
        for (const file of value.data.files) files.add(JSON.stringify([source.taskId, file.path]));
        if (value.diff) { additions += value.diff.additions; deletions += value.diff.deletions; hasDiff = true; partial ||= value.diff.truncated || value.diff.files.some(file => file.countsUnavailable); }
        else if (value.data.files.length) partial = true;
      } else partial = true;
      continue;
    }
    for (const file of source.files) {
      files.add(JSON.stringify([source.taskId, file.path]));
      if (!file.diff) { partial = true; continue; }
      const parsed = parseGitDiff(new TextEncoder().encode(reportedPatch(file.path, file.diff)));
      if (parsed.kind === "structured") {
        additions += parsed.diff.additions; deletions += parsed.diff.deletions; hasDiff = true; partial ||= parsed.diff.truncated;
      } else partial = true;
    }
  }
  // File count is unique paths; line counts sum the captured operations in this run.
  return { files: files.size, additions, deletions, hasDiff, partial };
}
