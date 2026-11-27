import type { InstallationSummary } from "@dogcodes/protocol";
import { agentAccessSupport, agentRuntimeSource } from "./agentRuntimeSource";
import { useI18n } from "./i18n";
import "./agentRuntimeBadge.css";

const supportCopy = {
  cli: { label: "agentUi.cliSupported", hint: "agentUi.cliOnlyHint" },
  desktop: { label: "agentUi.desktopSupported", hint: "agentUi.desktopOnlyHint" },
  "desktop-cli": { label: "agentUi.desktopCliSupported", hint: "agentUi.desktopOrCliHint" },
} as const;

/** Shows which install channels (desktop app, CLI, or both) an agent can be connected through. */
export function AgentAccessSupportBadge({ providerId }: { providerId: string }) {
  const { t } = useI18n();
  const support = agentAccessSupport(providerId);
  if (!support) return null;
  const copy = supportCopy[support];
  return <span className="agent-runtime-badge" data-runtime-support={support} data-tooltip={t(copy.hint)}>
    {t(copy.label)}
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
