import type { CoreClient, TimelineItem, TurnReviewRun, ReviewActivity as ReviewActivityItem } from "@dogcodes/protocol";
import { ActivityRow, isSubagent, type Activity } from "./ConversationActivity";
import { TurnProcess } from "./TurnProcess";
import { AgentIcon } from "./agentPresentation";
import { DeferredDetails } from "./DeferredDetails";
import { reviewActive } from "./reviewStream";
import { reviewActivityText } from "./reviewActivityText";

export function ReviewActivity({ client, run, notices = [] }: { client: CoreClient; run: TurnReviewRun; notices?: string[] }) {
  const activities = run.activities ?? [];
  const normalizedMessages = activities.some(item => !item.legacy && item.data.type === "message");
  const visible = activities.filter(item => !(item.legacy && item.data.type === "message" && normalizedMessages)).flatMap(item => {
    if (item.data.type !== "message") return [item];
    const text = reviewActivityText(String(item.data.text ?? ""));
    return text ? [{ ...item, data: { ...item.data, text } }] : [];
  });
  const messages = visible.filter(item => item.data.type === "message").map(item => String(item.data.text ?? ""));
  // Some drivers/older records only supply the final reply in result. Keep it in
  // the same transcript, without repeating a reply already received in the stream.
  const result = reviewActivityText(run.result);
  if (result && !messages.join("").includes(result)) {
    visible.push({ itemId: "review-final-output", data: { type: "message", text: result } });
  }
  if (!visible.length && !notices.length) return null;
  const active = reviewActive(run);
  const render = (entry: ReviewActivityItem) => {
    const item: TimelineItem = { itemId: entry.itemId, taskId: run.request.taskId, turnId: null, kind: "activity", text: JSON.stringify(entry.data), providerId: run.request.executor.providerId, sequence: 0, occurredAt: run.createdAt, attachments: [] };
    // Reviews have no main-conversation child session route. Show delegation as a
    // normal tool status instead of offering a link to an unrelated conversation.
    const a = entry.data as Activity;
    const display = isSubagent(a)
      ? { ...a, type: "tool", category: "review", name: "委派任务", input: a.title || a.input } : a;
    return <ActivityRow key={entry.itemId} item={item} a={display} active={active} client={client} onSubagent={() => {}}/>;
  };
  const isTool = (entry: ReviewActivityItem) => entry.data.type === "tool" || isSubagent(entry.data as Activity);
  const tools = visible.filter(isTool);
  const firstTool = tools[0];
  const content = () => <div className="review-activity" aria-label="审查执行过程">{visible.map(entry => isTool(entry)
    ? entry === firstTool && <TurnProcess key="review-tools" startedAt={run.createdAt} completedAt={run.finishedAt} active={active} agent={<AgentIcon providerId={run.request.executor.providerId}/>}>
        {() => tools.map(entry => <div className="conversation-entry" key={entry.itemId}>{render(entry)}</div>)}
      </TurnProcess>
    : render(entry))}{notices.map((notice, index) => <p className="turn-review-notice" role="status" key={`notice-${index}`}>{notice}</p>)}</div>;
  return run.status === "completed"
    ? <DeferredDetails className="review-execution" summary={<summary>审查执行过程</summary>}>{content}</DeferredDetails>
    : content();
}
