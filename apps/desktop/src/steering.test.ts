// @vitest-environment jsdom
import { expect, it } from "vitest";
import { sendSteering } from "./steering";

it("reuses the command after a lost receipt and allocates a new one after confirmation", async () => {
  const ids: string[] = [];
  await expect(sendSteering("tsk_test", "trn_test", "correction", async id => {
    ids.push(id);
    throw Error("lost receipt");
  })).rejects.toThrow("lost receipt");
  const send = async (id: string) => { ids.push(id); };
  await sendSteering("tsk_test", "trn_test", "correction", send);
  await sendSteering("tsk_test", "trn_test", "correction", send);
  expect(ids[0]).toBe(ids[1]);
  expect(ids[1]).not.toBe(ids[2]);
});
