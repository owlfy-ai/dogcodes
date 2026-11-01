import { useCallback, useEffect, useRef, useState } from "react";
import type { ExecutorRef, ModelCatalog, TaskId } from "@dogcodes/protocol";
import { ModelCatalogCache } from "./modelCatalogCache";
import { userFacingError } from "./userFacingError";
import { translate as t } from "./i18n";
const empty: ModelCatalog = { models: [], defaultModel: null };
export function useModelCatalog(cache: ModelCatalogCache, executor: ExecutorRef | undefined,
  version: string | null, taskId: TaskId | undefined, model: string | null, ready: boolean, busy: boolean) {
  const [state, setState] = useState({ owner: "", catalog: empty, loading: false, refreshing: false, error: null as string | null });
  const [revision, setRevision] = useState(0);
  const force = useRef(false);
  const owner = JSON.stringify([executor?.providerId, executor?.installationId]);
  const check = useCallback(() => setRevision(n => n + 1), []);
  const refresh = useCallback(() => { force.current = true; check(); }, [check]);
  useEffect(() => cache.subscribe(check), [cache, check]);
  useEffect(() => {
    const checkVisible = () => { if (document.visibilityState !== "hidden") check(); };
    window.addEventListener("focus", checkVisible); document.addEventListener("visibilitychange", checkVisible);
    // Only failed/missing scopes recover on this timer. Successful scopes never expire.
    const timer = window.setInterval(checkVisible, 60_000);
    return () => { window.removeEventListener("focus", checkVisible); document.removeEventListener("visibilitychange", checkVisible); window.clearInterval(timer); };
  }, [check]);
  const fallback = executor ? cache.fallback(executor, version, { model, taskId }) : undefined;
  useEffect(() => {
    if (!executor || executor.providerId === "fake") return;
    const scope = { model, taskId }, cached = cache.fallback(executor, version, scope);
    const savedError = cache.error(executor, version, scope);
    const error = savedError ? userFacingError(savedError, t("modelPicker.listFailed")) : null;
    if (!ready || busy || document.visibilityState === "hidden") {
      setState(previous => ({ owner, catalog: cached ?? (previous.owner === owner ? previous.catalog : empty), loading: !cached && !busy, refreshing: false, error })); return;
    }
    const requestedRefresh = force.current; force.current = false;
    if (!requestedRefresh && !cache.needsRead(executor, version, scope)) {
      setState({ owner, catalog: cached ?? empty, loading: false, refreshing: false, error }); return;
    }
    let cancelled = false;
    setState({ owner, catalog: cached ?? empty, loading: !cached, refreshing: !!cached, error: null });
    cache.get(executor, version, requestedRefresh, scope).then(catalog => {
      if (!cancelled) setState({ owner, catalog, loading: false, refreshing: false, error: null });
    }, cause => {
      if (!cancelled) setState(previous => ({ ...previous, loading: false, refreshing: false, error: userFacingError(cause, t("modelPicker.listFailed")) }));
    });
    return () => { cancelled = true; };
  }, [cache, owner, executor?.providerId, executor?.installationId, version, taskId, model, ready, busy, revision]);
  return { catalog: state.owner === owner ? state.catalog : fallback ?? empty,
    loading: executor?.providerId !== "fake" && (state.owner === owner ? state.loading : !fallback),
    refreshing: state.owner === owner && state.refreshing,
    hasCatalog: !!fallback || (state.owner === owner && state.catalog !== empty),
    error: state.owner === owner ? state.error : null, refresh, check };
}
