import { userFacingError } from "./userFacingError";
import { translate as t } from "./i18n";


/** Carries Git's own output, for the person to read behind a disclosure. */
export class GitCommandFailed extends Error {}

export function reviewGitError(error: string) {
  if (error.includes("REVIEW_NO_UPSTREAM")) return t("gitError.noUpstream");
  if (/Please tell me who you are|empty ident name|unable to auto-detect email address/i.test(error)) return t("gitError.noIdentity");
  if (error.includes("REVIEW_DETACHED_HEAD")) return t("gitError.detachedHead");
  if (/CONFLICT \(|unmerged files|unresolved conflict|not concluded your merge/i.test(error)) return t("gitError.conflict");
  if (/would be overwritten|cannot pull with rebase.*changes/i.test(error)) return t("gitError.localChanges");
  if (/Need to specify how to reconcile divergent branches/i.test(error)) return t("gitError.diverged");
  if (error.startsWith("REVIEW_SYNC_PUSH_FAILED")) return t("gitError.syncPushFailed");
  // Callers prefix push failures after a commit with this translated marker.
  const committed = error.startsWith(t("gitCommit.pushFailedPrefix"));
  if (committed) {
    if (/fetch first|non-fast-forward|remote contains work|behind.*remote/i.test(error)) {
      return t("gitError.remoteAhead");
    }
    return t("gitError.pushFailed");
  }
  if (error.includes("REVIEW_STALE_COMMIT")) return t("gitError.staleCommit");
  if (error.includes("REVIEW_NO_CHANGES")) return t("gitError.noChanges");
  return t("gitError.unknown");
}

export type ReviewGitFailure = { message: string; detail?: string };

/** Git rejects with its own output (a string), kept behind a disclosure; other failures show only a cleaned message. */
export function reviewGitFailure(cause: unknown, fallback: string, prefix = ""): ReviewGitFailure {
  const output = cause instanceof GitCommandFailed ? cause.message : typeof cause === "string" ? cause : "";
  if (output.trim()) { const detail = prefix + output; return { message: reviewGitError(detail), detail }; }
  return { message: userFacingError(cause, fallback) };
}
