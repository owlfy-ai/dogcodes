import { useEffect, useState, type ReactNode } from "react";
import { ActivityLoader } from "./ActivityLoader";

export function StartupShell({ children }: { children?: ReactNode }) {
  return <main className="startup-shell" aria-label="dogcodes">
    <aside className="startup-sidebar" aria-hidden="true"><header className="startup-titlebar" data-tauri-drag-region="deep" /><div className="startup-lines"><i /><i /><i /><i /><i /></div></aside>
    <section className="startup-content"><header className="startup-titlebar" data-tauri-drag-region="deep" />{children ?? <StartupPresence />}</section>
  </main>;
}
function StartupPresence() {
  const [slow, setSlow] = useState(false);
  useEffect(() => { const timer = setTimeout(() => setSlow(true), 12000); return () => clearTimeout(timer); }, []);
  return <div className="startup-presence"><div role="status" aria-label="正在打开 dogcodes"><ActivityLoader /></div>
    {slow && <div className="startup-help" role="status"><p>打开时间比平时久一些，可以重新尝试。</p><button type="button" onClick={() => window.location.reload()}>重新打开</button></div>}
  </div>;
}
