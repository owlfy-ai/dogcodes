import type { InstallationSummary } from "@dogcodes/protocol";
import { agentRequiresCli, agentRuntimeSource } from "./agentRuntimeSource";
import { useI18n } from "./i18n";
import "./agentRuntimeBadge.css";

export function AgentCliSupportBadge({ providerId }: { providerId: string }) {
  const { t } = useI18n();
  if (!agentRequiresCli(providerId)) return null;
  return <span className="agent-runtime-badge" data-runtime-support="cli" data-tooltip={t("agentUi.cliOnlyHint")}>
    {t("agentUi.cliSupported")}
  </span>;
}

export function AgentRuntimeBadge({ installation }: { installation?: Partial<Pick<InstallationSummary, "providerId" | "executable">> }) {
  const { t } = useI18n();
  if (!installation?.providerId || !installation.executable) return null;
  const source = agentRuntimeSource({ providerId: installation.providerId, executable: installation.executable });
  if (!source) return null;
  return <span className="agent-runtime-badge" data-runtime-source={source}
    data-tooltip={`${t("agentUi.installationPath")}\n${installation.executable}`}>
    {t(source === "desktop" ? "agentUi.desktopApp" : "agentUi.runtimeCli")}
  </span>;
}
