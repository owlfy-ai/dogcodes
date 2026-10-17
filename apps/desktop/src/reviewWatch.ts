import { createInvalidationSubscriber } from "./invalidationWatch";
const subscribe = createInvalidationSubscriber("core_review_watch", "core_review_unwatch");
/** One stream per task per JS context, shared by undo cards and recovery panels. */
export function subscribeReviewChanges(taskId: string, refresh: () => void, error?: (message: string) => void): () => void {
  return subscribe(taskId, { taskId }, refresh, error);
}
