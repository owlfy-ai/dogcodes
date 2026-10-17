import { describe, expect, it, vi } from "vitest";
import { DemoCoreClient } from "./demoClient";
import { submitWithAutomaticContext } from "./automaticContext";

describe("automatic context delivery", () => {
  it("freezes context and confirms the executor before sending the prompt", async () => {
    const client = new DemoCoreClient();
    const task = (await client.bootstrap()).tasks[0];
    const calls: string[] = [];
    const confirm = client.confirmContextPackage.bind(client);
    vi.spyOn(client, "confirmContextPackage").mockImplementation(async (...args) => {
      calls.push("context");
      await confirm(...args);
    });
    vi.spyOn(client, "submitTurn").mockImplementation(async (current, prompt) => {
      calls.push("prompt");
      expect(current.preferredExecutor.providerId).toBe("fake-peer");
      expect(prompt).toBe("continue");
    });
    await submitWithAutomaticContext(client, task.taskId,
      { providerId: "fake-peer", installationId: null }, "continue", null);
    expect(calls).toEqual(["context", "prompt"]);
  });

  it("does not send the user prompt when context preparation fails", async () => {
    const client = new DemoCoreClient();
    const task = (await client.bootstrap()).tasks[0];
    vi.spyOn(client, "buildContextPreview").mockRejectedValue(new Error("failed"));
    const send = vi.spyOn(client, "submitTurn");
    await expect(submitWithAutomaticContext(client, task.taskId,
      { providerId: "fake-peer", installationId: null }, "continue", null)).rejects.toThrow("failed");
    expect(send).not.toHaveBeenCalled();
  });

  it("continues on the same executor without building another package", async () => {
    const client = new DemoCoreClient();
    const task = (await client.bootstrap()).tasks[0];
    const build = vi.spyOn(client, "buildContextPreview");
    vi.spyOn(client, "submitTurn").mockResolvedValue(undefined);
    await submitWithAutomaticContext(client, task.taskId, task.preferredExecutor, "continue", null);
    expect(build).not.toHaveBeenCalled();
  });
});

it("keeps image and file references separate from user prompt text", async () => {
  const client = new DemoCoreClient();
  const task = (await client.bootstrap()).tasks[0];
  const send = vi.spyOn(client, "submitTurn").mockResolvedValue(undefined);
  const attachments = [{name: "clipboard.png", path: "/app/attachments/clipboard.png", mimeType: "image/png"}];
  await submitWithAutomaticContext(client, task.taskId, task.preferredExecutor, "看看这张图", null, null, null, attachments);
  expect(send).toHaveBeenCalledWith(expect.anything(), "看看这张图", null, null, null, attachments);
});
