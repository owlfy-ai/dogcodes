import { invoke, isTauri } from "@tauri-apps/api/core";
import { useRef, useState } from "react";
import type { InstallationSummary, QuotaSnapshotView } from "@dogcodes/protocol";
import { LoadingStatus } from "./LoadingStatus";
import { userFacingError } from "./userFacingError";
import "./agentLogin.css";

type AccountInstallation = Partial<Pick<InstallationSummary, "status" | "version" | "diagnostic">>;
export function agentAccountIssue(installation: AccountInstallation | undefined, view?: QuotaSnapshotView): { label: string; detail: string; loginRequired: boolean } | null {
  if (!installation || installation.version === "demo" || view?.quota?.confidence === "demo") return null;
  for (const code of [view?.diagnostic, installation.diagnostic]) {
    switch (code) {
      case "claude_quota_login_required": return { label: "未登录订阅账号", detail: "订阅授权缺失、失效或权限不足；API Key 仍可能支持对话。", loginRequired: true };
      case "authentication_failed":
      case "quota_login_required": return { label: "未登录", detail: "登录授权缺失、失效或被拒绝，请检查原生客户端登录状态或 API Key 后重新检测。", loginRequired: true };
      case "subscription_required":
      case "quota_subscription_required": return { label: "未订阅或已到期", detail: "当前账号没有有效订阅，请在原生客户端确认套餐或切换有权限的账号、模型。", loginRequired: false };
      case "access_denied":
      case "quota_access_denied": return { label: "无访问权限", detail: "当前账号未获得服务或模型访问权限，请检查账号资格及服务配置。", loginRequired: false };
      case "quota_not_configured":
      case "driver_not_configured": return { label: "未配置", detail: "Agent 配置尚未就绪，请在原生客户端检查模型、服务及运行环境后重新检测。", loginRequired: false };
    }
  }
  return installation.status === "not_configured"
    ? { label: "未配置", detail: "Agent 配置尚未就绪，请在原生客户端检查模型、服务及运行环境后重新检测。", loginRequired: false }
    : null;
}
export function needsAgentLogin(installation: AccountInstallation | undefined, view?: QuotaSnapshotView) {
  return agentAccountIssue(installation, view)?.loginRequired ?? false;
}

export function AgentLogin({ installationId, name }: {
  installationId: string; name: string;
}) {
  const busy = useRef(false);
  const [opening, setOpening] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const open = async () => {
    if (busy.current) return;
    if (!isTauri()) { setError("请在 dog.codes 桌面端打开登录入口。"); return; }
    busy.current = true;
    setOpening(true); setError(null);
    try {
      await invoke("open_agent_login", { installationId });
    } catch (cause) {
      setError(userFacingError(cause, "无法打开登录入口，请重试或手动打开客户端。"));
    } finally {
      busy.current = false;
      setOpening(false);
    }
  };
  return <span className="agent-login">
    <button type="button" className="agent-login-button" onClick={() => void open()} disabled={opening}
      aria-label={`${name}：去登录`} aria-busy={opening} data-tooltip={error || undefined}>
      {opening ? <LoadingStatus>打开中</LoadingStatus> : error ? "重试" : "去登录"}
    </button>
    {error && <span className="agent-login-feedback" role="alert">{error}</span>}
  </span>;
}
