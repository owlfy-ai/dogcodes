import { expect, it, vi } from "vitest";
import type { CoreClient, PermissionCatalog } from "@dogcodes/protocol";
import { permissionCatalogEntry } from "./permissionCatalogCache";
const executor = { providerId: "kimi", installationId: "kimi-a" };
const catalog: PermissionCatalog = { defaultModeId: "default", modes: [] };
it("shares pending reads and reuses successful lists across picker remounts", async () => {
  const read = vi.fn(async () => catalog);
  const client = { permissionCatalog: read } as unknown as CoreClient;
  const first = permissionCatalogEntry(client, executor, "tsk_a", "1").get();
  expect(permissionCatalogEntry(client, executor, "tsk_a", "1").get()).toBe(first);
  await first;
  expect(permissionCatalogEntry(client, executor, "tsk_a", "1").peek()).toBe(catalog);
  await permissionCatalogEntry(client, executor, "tsk_a", "1").get();
  expect(read).toHaveBeenCalledTimes(1);
  await permissionCatalogEntry(client, executor, "tsk_b", "1").get();
  await permissionCatalogEntry(client, executor, "tsk_a", "2").get();
  expect(read).toHaveBeenCalledTimes(2);
});
it("does not retain busy errors or expire lists just because time passes", async () => {
  vi.useFakeTimers();
  try {
    const read = vi.fn().mockRejectedValueOnce(Error("busy")).mockResolvedValue(catalog);
    const client = { permissionCatalog: read } as unknown as CoreClient;
    await expect(permissionCatalogEntry(client, executor).get()).rejects.toThrow("busy");
    await permissionCatalogEntry(client, executor).get();
    vi.advanceTimersByTime(3600_000);
    expect(permissionCatalogEntry(client, executor).peek()).toBe(catalog);
    await permissionCatalogEntry(client, executor).get();
    expect(read).toHaveBeenCalledTimes(2);
  } finally { vi.useRealTimers(); }
});
