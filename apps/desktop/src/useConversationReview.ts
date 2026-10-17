import { useCallback, useState, type SetStateAction } from "react";

/** Share the sidebar toggle across conversations for this app session. */
export function useConversationReview(taskId: string | null) {
  const [open, setOpenState] = useState(false);
  const setOpen = useCallback((value: SetStateAction<boolean>) => {
    if (!taskId) return;
    setOpenState(value);
  }, [taskId]);
  // Drafts have no reader; hide it without resetting the user's global toggle.
  return [taskId !== null && open, setOpen] as const;
}
