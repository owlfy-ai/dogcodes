// Window previews after computer-use calls. They live only in this window's
// memory and only the newest picture is kept: it is shown while its turn runs.
import { useSyncExternalStore } from "react";
import { isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export type ComputerUsePreview = {
  itemId: string;
  taskId: string;
  turnId?: string;
  image?: string;
  marker?: { x: number; y: number };
  window?: string;
  element?: string;
  windows?: string[];
};

const LIMIT = 40;
const previews = new Map<string, ComputerUsePreview>();
let latest: ComputerUsePreview | undefined;
const listeners = new Set<() => void>();
let listening = false;

export function receivePreview(preview: ComputerUsePreview) {
  if (!preview || typeof preview.itemId !== "string") return;
  if (preview.image && !/^data:image\/(png|jpeg|webp);base64,/.test(preview.image)) delete preview.image;
  if (preview.image) latest = preview;
  // Rows keep only the text description; the picture lives in `latest`.
  const { image: _image, marker: _marker, ...description } = preview;
  previews.delete(preview.itemId);
  previews.set(preview.itemId, description);
  // Bounded: descriptions of older calls are simply dropped.
  while (previews.size > LIMIT) previews.delete(previews.keys().next().value!);
  listeners.forEach(listener => listener());
}

function start() {
  if (listening || !isTauri()) return;
  listening = true;
  void listen<ComputerUsePreview>("computer-use-preview", event => receivePreview(event.payload))
    .catch(() => { listening = false; });
}

function subscribe(listener: () => void) {
  start();
  listeners.add(listener);
  return () => { listeners.delete(listener); };
}

export function useComputerUsePreview(itemId: string | undefined) {
  return useSyncExternalStore(subscribe, () => (itemId ? previews.get(itemId) : undefined));
}

/** The newest picture, only while that task's turn is the running one. */
export function useLatestComputerUsePreview(taskId: string | undefined, turnId: string | undefined) {
  const current = useSyncExternalStore(subscribe, () => latest);
  return current && taskId && turnId && current.taskId === taskId && current.turnId === turnId ? current : undefined;
}

export function clearPreviewsForTest() {
  previews.clear();
  latest = undefined;
}
