import { type ReactNode, Suspense, lazy } from "react";
import { LoadingStatus } from "./LoadingStatus";
import { useI18n } from "./i18n";
import { hostTerminalScope, type TerminalApi, type TerminalStatus } from "./terminalClient";

const TerminalPane = lazy(() => import("./TerminalDock").then(module => ({ default: module.TerminalPane })));

/** One tab of a host: its connection until there is one, then a shell. A shell that
 * has started stays on screen after it ends, so its output can still be read. */
export function HostTerminalTab({ targetId, api, operationId, resourceId, status, connected, visible, connecting, initialCommand, onStatus, onReconnect }: {
  targetId: string; api: TerminalApi; operationId: string; resourceId?: string; status?: TerminalStatus; connected: boolean; visible: boolean;
  connecting: ReactNode; initialCommand?: string; onStatus: (status: TerminalStatus, epoch: string) => void; onReconnect: () => void;
}) {
  const { t } = useI18n();
  const ended = status === "exited" || status === "failed" || status === "unknown";
  return <div className="target-terminal-tab" hidden={!visible}>
    {status || connected
      ? <div className="terminal-host">
        <Suspense fallback={<LoadingStatus>{t("app.loadingTerminal")}</LoadingStatus>}>
          <TerminalPane key={operationId} projectId={hostTerminalScope(targetId)} directoryIndex={0} operationId={operationId} resourceId={resourceId} visible={visible} api={api} onStatus={onStatus} initialCommand={initialCommand} />
        </Suspense>
        {ended && <div className="target-terminal-ended">
          <span>{t("executionTargets.sessionEnded")}</span>
          <button type="button" onClick={onReconnect}>{t("executionTargets.reconnect")}</button>
        </div>}
      </div>
      : <div className="target-terminal-connecting" role="tabpanel">{connecting}</div>}
  </div>;
}
