import "./activityPresentation.css";

export function ActivityIcon({ kind }: { kind: string }) {
  const shape = kind === "思考" ? <><path d="M9 18h6m-5 3h4M8.5 14.5a6 6 0 1 1 7 0c-.9.6-1.5 1.5-1.5 2.5h-4c0-1-.6-1.9-1.5-2.5Z" /></>
    : kind === "终端" ? <><rect x="3" y="4" width="18" height="16" rx="3" /><path d="m7 9 3 3-3 3m6 0h4" /></>
    : kind === "读取" ? <><path d="M14 3H6a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V9l-6-6Z" /><path d="M14 3v6h6M8 13h8m-8 4h6" /></>
    : kind === "搜索" || kind === "查阅" ? <><circle cx="10.5" cy="10.5" r="6.5" /><path d="m16 16 5 5" /></>
    : kind === "网页搜索" || kind === "获取网页" ? <><circle cx="12" cy="12" r="9" /><path d="M3 12h18M12 3a17 17 0 0 1 0 18 17 17 0 0 1 0-18Z" /></>
    : kind === "修改" ? <><path d="m15 4 5 5M4 20l5-1L20 8a2.1 2.1 0 0 0-4-4L5 15l-1 5Z" /></>
    : kind === "子智能体" ? <><rect x="4" y="7" width="16" height="13" rx="3" /><path d="M12 3v4M2 12v4m20-4v4M8 12h.01M16 12h.01M9 16h6" /></>
    : <path d="M14 4a6 6 0 0 0-7 7L3.5 14.5a3.2 3.2 0 0 0 4.5 4.5l3.5-3.5a6 6 0 0 0 7-7L15 12l-3-3 3.5-3.5Z" />;
  return <svg className="activity-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">{shape}</svg>;
}
