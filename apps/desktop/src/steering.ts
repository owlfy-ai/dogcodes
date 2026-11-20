import { ulid } from "ulid";
import type { TaskId, TurnId } from "@dogcodes/protocol";

/** Keep a retry tied to the same host receipt after a lost IPC response or window reload. */
export async function sendSteering(taskId: TaskId, turn: TurnId, text: string, send: (id: string) => Promise<void>) {
  const key = `dogcodes.steer.${taskId}`;
  const previous = JSON.parse(sessionStorage.getItem(key) ?? "null");
  const id = previous?.turn === turn && previous.text === text ? previous.id : `cmd_${ulid()}`;
  const record = JSON.stringify({ id, turn, text });
  sessionStorage.setItem(key, record);
  await send(id);
  if (sessionStorage.getItem(key) === record) sessionStorage.removeItem(key);
}
