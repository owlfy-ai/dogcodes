import { Component, type ReactNode } from "react";
import { StartupShell } from "./StartupShell";

export class AppErrorBoundary extends Component<{ children: ReactNode }, { failed: boolean }> {
  state = { failed: false };
  static getDerivedStateFromError() { return { failed: true }; }
  render() {
    return this.state.failed ? <StartupShell><div className="startup-help" role="alert"><h2>页面暂时无法显示</h2><p>请重新打开页面。如果问题持续出现，请退出后重新打开应用。</p><button type="button" onClick={() => window.location.reload()}>重新打开</button></div></StartupShell> : this.props.children;
  }
}
