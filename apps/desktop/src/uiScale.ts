export const MIN_UI_SCALE = 70;
export const MAX_UI_SCALE = 150;
export type ScaleAction = "in" | "out" | "reset";

export function parseUiScale(value: string | null): number {
  const numeric = Number(value);
  return value?.trim() && Number.isFinite(numeric)
    ? Math.min(MAX_UI_SCALE, Math.max(MIN_UI_SCALE, Math.round(numeric / 10) * 10))
    : 100;
}

export function adjustUiScale(value: number, action: ScaleAction): number {
  return action === "reset" ? 100 : Math.min(MAX_UI_SCALE, Math.max(MIN_UI_SCALE, value + (action === "in" ? 10 : -10)));
}

export function scaleShortcut(event: Pick<KeyboardEvent, "key" | "metaKey" | "ctrlKey" | "altKey" | "isComposing">): ScaleAction | null {
  if (!(event.metaKey || event.ctrlKey) || event.altKey || event.isComposing) return null;
  if (event.key === "+" || event.key === "=") return "in";
  if (event.key === "-") return "out";
  return event.key === "0" ? "reset" : null;
}
