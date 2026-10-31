import { useCallback, useEffect, useRef, useState } from "react";
import type { ExecutorRef, ModelCatalog, TaskId } from "@dogcodes/protocol";
import { ModelCatalogCache } from "./modelCatalogCache";
import { userFacingError } from "./userFacingError";
import { translate as t } from "./i18n";

const empty: ModelCatalog = { models: [], defaultModel: null };
export function useModelCatalog(cache: ModelCatalogCache, executor: ExecutorRef | undefined,
  version: string | null, taskId: TaskId | undefined, model: string | null, ready: boolean, busy: boolean) {
  const [state, setState] = useState({ owner: "", catalog: empty, loading: false, error: null as string | null });
  const [revision, setRevision] = useState(0);
  const force = useRef(false);
  const owner = JSON.stringify([executor?.providerId, executor?.installationId, taskId]);
  const check = useCallback(() => setRevision(n => n + 1), []);
  const refresh = useCallback(() => { force.current = true; check(); }, [check]);
  useEffect(() => {
    const checkVisible = () => { if (document.visibilityState !== "hidden") check(); };
    window.addEventListener("focus", checkVisible);
    document.addEventListener("visibilitychange", checkVisible);
    const timer = window.setInterval(checkVisible, 60_000);
    return () => {
      window.removeEventListener("focus", checkVisible);
      document.removeEventListener("visibilitychange", checkVisible);
      window.clearInterval(timer);
    };
  }, [check]);
  useEffect(() => {
    if (!executor || executor.providerId === "fake") return;
    if (!ready || busy || document.visibilityState === "hidden") return;
    let cancelled = false;
    const scope = { model, taskId };
    const refresh = force.current;
    force.current = false;
    const cached = refresh ? undefined : cache.peek(executor, version, scope);
    if (cached) {
      setState({ owner, catalog: cached, loading: false, error: null });
      return;
    }
    setState(previous => ({ owner, catalog: cache.peek(executor, version, scope, true)
      ?? (previous.owner === owner ? previous.catalog : empty), loading: true, error: null }));
    cache.get(executor, version, refresh, scope).then(catalog => {
      if (!cancelled) setState({ owner, catalog, loading: false, error: null });
    }, error => {
      if (!cancelled) setState(previous => ({ ...previous, loading: false, error: userFacingError(error, t("modelPicker.listFailed")) }));
    });
    return () => { cancelled = true; };
  }, [cache, owner, executor?.providerId, executor?.installationId, version, taskId, model, ready, busy, revision]);
  return { catalog: state.owner === owner ? state.catalog : empty,
    loading: executor?.providerId !== "fake" && (!ready || (state.owner === owner && state.loading)),
    error: state.owner === owner ? state.error : null, refresh, check };
}
