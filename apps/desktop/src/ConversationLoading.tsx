import { ActivityLoader } from "./ActivityLoader";

/** The main area's loader; its parent times it with useLoadingVisibility, so fast switches stay blank. */
export function ConversationLoading({ visible }: { visible: boolean }) {
  return <div className="conversation-loading">{visible && <span className="conversation-loading-mark" role="status"><ActivityLoader /><span className="conversation-loading-label">正在加载对话…</span></span>}</div>;
}
