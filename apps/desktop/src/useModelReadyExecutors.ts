import { useEffect, useState } from "react";
import type { CoreClient } from "@dogcodes/protocol";

export function useModelReadyExecutors(client: CoreClient) {
  // Persisted installation readiness is not runtime driver readiness. On restart
  // native probes may still be running, so only warm registered drivers.
  const [modelReadyIds, setModelReadyIds] = useState<string[]>([]);
  useEffect(() => {
    let active = true;
    let stop: (() => void) | undefined;
    const load = () => {
      void client.modelReadyExecutors?.().then(executors => {
        if (active) setModelReadyIds(executors.map(ex => `${ex.providerId}:${ex.installationId}`));
      }).catch(() => {});
    };
    setModelReadyIds([]);
    void client.onQuotaProviderReady?.(load).then(unlisten => {
      if (active) { stop = unlisten; load(); } else unlisten();
    }).catch(() => {});
    load();
    // Also recover from a missed native event or a temporary IPC failure.
    const timer = setInterval(load, 5000);
    return () => { active = false; stop?.(); clearInterval(timer); };
  }, [client]);
  return modelReadyIds;
}
