import { userFacingError } from "./userFacingError";


export function reviewGitError(error: string) {
  if (error.includes("REVIEW_NO_UPSTREAM")) return "当前分支没有配置上游，请先发布分支或设置上游后重试。";
  if (error.includes("REVIEW_DETACHED_HEAD")) return "当前未处于分支上，请先切换到需要同步的分支。";
  if (/CONFLICT \(|unmerged files|unresolved conflict|not concluded your merge/i.test(error)) return "存在合并冲突，请解决冲突并完成合并或变基后再同步。";
  if (/would be overwritten|cannot pull with rebase.*changes/i.test(error)) return "本地更改阻止了拉取，请先提交或暂存到 stash 后重试。";
  if (/Need to specify how to reconcile divergent branches/i.test(error)) return "分支已分叉，请先在 Git 中配置拉取时使用合并或变基后重试。";
  if (error.startsWith("REVIEW_SYNC_PUSH_FAILED")) return "拉取已完成，推送失败。请查看详情，处理后再同步。";
  const committed = error.startsWith("提交已完成，推送失败：");
  if (committed) {
    if (/fetch first|non-fast-forward|remote contains work|behind.*remote/i.test(error)) {
      return "本地提交已完成。远端有新提交，请先同步远端并处理可能的冲突，再重试推送。";
    }
    return "提交已完成，推送失败。请查看详情，处理后重试推送。";
  }
  if (error.includes("REVIEW_STALE_COMMIT")) return "Git 状态已变化，文件可能已在其他地方提交或修改。请检查最新差异后再提交。";
  if (error.includes("REVIEW_NO_CHANGES")) return "当前没有可提交的更改。";
  return "操作未完成，请展开详情查看原因。";
}

export type ReviewGitFailure = { message: string; detail?: string };

/** Git rejects with its own output (a string), kept behind a disclosure; other failures show only a cleaned message. */
export function reviewGitFailure(cause: unknown, fallback: string, prefix = ""): ReviewGitFailure {
  if (typeof cause === "string" && cause.trim()) { const detail = prefix + cause; return { message: reviewGitError(detail), detail }; }
  return { message: userFacingError(cause, fallback) };
}
