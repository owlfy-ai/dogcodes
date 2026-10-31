import { Component, type ReactNode } from "react";
import { StartupShell } from "./StartupShell";
import { translate as t } from "./i18n";

export class AppErrorBoundary extends Component<{ children: ReactNode }, { failed: boolean }> {
  state = { failed: false };
  static getDerivedStateFromError() { return { failed: true }; }
  render() {
    return this.state.failed ? <StartupShell><div className="startup-help" role="alert"><h2>{t("errorBoundary.title")}</h2><p>{t("errorBoundary.description")}</p><button type="button" onClick={() => window.location.reload()}>{t("errorBoundary.reload")}</button></div></StartupShell> : this.props.children;
  }
}
