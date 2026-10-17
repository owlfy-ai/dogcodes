import type { ContextPackageV1 } from "@dogcodes/protocol";

export interface ContextEditDiff {
  field: "目标" | "已完成" | "待办";
  before: string;
  after: string;
}

const editableValues = (value: ContextPackageV1) => [
  value.objective,
  value.completedWork.join("\n"),
  value.pendingWork.join("\n"),
] as const;

export function sameEditableContext(left: ContextPackageV1, right: ContextPackageV1): boolean {
  return editableValues(left).every((value, index) => value === editableValues(right)[index]);
}

export function contextEditDiff(
  original: ContextPackageV1,
  finalVersion: ContextPackageV1,
): ContextEditDiff[] {
  const labels = ["目标", "已完成", "待办"] as const;
  const before = editableValues(original);
  const after = editableValues(finalVersion);
  return labels.flatMap((field, index) => before[index] === after[index]
    ? []
    : [{ field, before: before[index], after: after[index] }]);
}

export function editableDraftFromSafePreview(
  safePreview: ContextPackageV1,
  original: ContextPackageV1,
): ContextPackageV1 {
  const draft = structuredClone(safePreview);
  draft.provenance = structuredClone(original.provenance);
  return draft;
}
