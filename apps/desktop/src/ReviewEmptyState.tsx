import type { ReactNode } from "react";
import "./emptyState.css";

export function ReviewEmptyState({ kind = "changes", title = kind === "run" ? "暂无运行命令" : "没有变更", description, action, compact = false }: {
  kind?: "changes" | "run" | "terminal" | "folder" | "search" | "history" | "file";
  title?: string;
  description?: string;
  action?: ReactNode;
  compact?: boolean;
} = {}) {
  return <div className={`review-empty-state${compact ? " review-empty-state--compact" : ""}`} role="status">
    <div className="review-empty-content">
      <div className="review-empty-art" aria-hidden="true">
        <svg viewBox="0 0 144 124" fill="none">
          <circle className="review-empty-orbit" cx="72" cy="62" r="53" strokeDasharray="2 6" />
          {kind === "terminal" ? <>
            <path className="review-empty-sheet-back" d="M62 87v13h20V87M53 101h38" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" />
            <rect className="review-empty-sheet" x="24" y="27" width="96" height="61" rx="8" />
            <path className="review-empty-sheet" d="M24 42h96" />
            <path className="review-empty-line" d="M35 35h1m7 0h1m7 0h1" strokeLinecap="round" />
            <g className="review-empty-branch" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round">
              <path d="m40 55 9 8-9 8m18 0h15" />
              <path d="M83 56v15" opacity=".4" />
            </g>
          </> : kind === "run" ? <>
            <rect className="review-empty-sheet-back" x="25" y="28" width="80" height="66" rx="9" transform="rotate(-10 65 61)" />
            <rect className="review-empty-sheet" x="32" y="24" width="80" height="66" rx="9" />
            <path className="review-empty-sheet" d="M32 43h80" />
            <path className="review-empty-line" d="M44 34h1m7 0h1m7 0h1" strokeLinecap="round" />
            <g className="review-empty-branch" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
              <path d="m47 56 8 7-8 7m17 0h17" />
            </g>
          </> : kind === "folder" ? <>
            <rect className="review-empty-sheet-back" x="31" y="31" width="74" height="58" rx="8" transform="rotate(-10 68 60)" />
            <path className="review-empty-sheet" d="M32 43V34a7 7 0 0 1 7-7h19l9 10h35a7 7 0 0 1 7 7v39a7 7 0 0 1-7 7H39a7 7 0 0 1-7-7V43Z" />
            <path className="review-empty-line" d="M46 57h46M46 66h26" strokeLinecap="round" />
          </> : <>
            <rect className="review-empty-sheet-back" x="37" y="22" width="62" height="78" rx="10" transform="rotate(-10 68 61)" />
            <rect className="review-empty-sheet" x="44" y="20" width="62" height="80" rx="10" />
            <path className="review-empty-line" d="M60 37h29M60 45h18" strokeLinecap="round" />
            <g className="review-empty-branch" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
              {kind === "changes" ? <><path d="M63 62v18m4-3c16 0 20-5 20-15" />
                <circle cx="63" cy="59" r="3.5" /><circle cx="63" cy="84" r="3.5" /><circle cx="87" cy="59" r="3.5" /></>
                : kind === "search" ? <><circle cx="72" cy="66" r="10" /><path d="m80 74 10 10" /></>
                : kind === "history" ? <><circle cx="75" cy="70" r="13" /><path d="M75 62v9l6 4" /></>
                : <path className="review-empty-line" d="M60 61h29M60 69h20M60 77h24" />}
            </g>
          </>}
          <circle className="review-empty-badge" cx="105" cy="95" r="16" />
          {kind === "run"
            ? <path className="review-empty-check" d="m101 88 11 7-11 7Z" strokeWidth="2" strokeLinejoin="round" />
            : kind === "changes" ? <path className="review-empty-check" d="m98.5 95 4.5 4.5 8-9" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round" />
            : <path className="review-empty-check" d={kind === "folder" || kind === "terminal" ? "M99 95h12m-6-6v12" : "M100 95h10"} strokeWidth="2" strokeLinecap="round" />}
        </svg>
      </div>
      <h3>{title}</h3>
      {description && <p>{description}</p>}
      {action && <div className="review-empty-actions">{action}</div>}
    </div>
  </div>;
}
