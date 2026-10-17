import { describe, expect, it } from "vitest";
import type { ContextPackageV1 } from "@dogcodes/protocol";
import { contextEditDiff, editableDraftFromSafePreview, sameEditableContext } from "./contextEdit";

const contextPackage = (): ContextPackageV1 => ({
  schemaVersion: 1, deliveryFacts: [], conversationText: "",
  kind: "handoff",
  strength: "standard",
  sourceExecutor: { providerId: "a", installationId: null },
  targetExecutor: { providerId: "b", installationId: null },
  objective: "Ship it",
  userConstraints: [],
  acceptanceCriteria: [],
  completedWork: ["Built preview"],
  decisions: [],
  pendingWork: [],
  relevantFiles: [],
  dependencyChanges: [],
  changeSummary: { base: "unknown", head: "unknown", diffStat: "unknown", artifactRef: null },
  tests: [],
  recentTurnRefs: [],
  contextPointers: [],
  semanticSummary: null,
  provenance: {
    builderVersion: 1,
    builtAt: "2026-09-02T00:00:00Z",
    redactionRulesetVersion: 1,
    redactionCount: 0,
    sourceAfterSequence: 0,
    sourceThroughSequence: 4,
    sourceSequences: [1, 2, 3, 4],
  },
});

describe("context edit presentation", () => {
  it("reports only editable field changes", () => {
    const original = contextPackage();
    const edited = structuredClone(original);
    edited.objective = "Ship safely";
    edited.pendingWork = ["Run checks"];
    expect(contextEditDiff(original, edited)).toEqual([
      { field: "目标", before: "Ship it", after: "Ship safely" },
      { field: "待办", before: "", after: "Run checks" },
    ]);
    expect(sameEditableContext(original, edited)).toBe(false);
  });

  it("uses safe text in the next editor while restoring immutable base provenance", () => {
    const original = contextPackage();
    const safe = structuredClone(original);
    safe.pendingWork = ["Authorization: Bearer [REDACTED]"];
    safe.provenance.redactionCount = 1;
    const nextDraft = editableDraftFromSafePreview(safe, original);
    expect(nextDraft.pendingWork[0]).toContain("[REDACTED]");
    expect(nextDraft.provenance).toEqual(original.provenance);
  });
});
