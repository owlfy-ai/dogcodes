import { useEffect, useState } from "react";
import { DialogButton } from "./AppDialog";
import { installFailure, laterVersion, rememberLater, shouldOfferUpdate, tauriAppUpdateApi, type AppUpdate, type AppUpdateApi, type AppUpdateFailure } from "./appUpdate";
import { useI18n } from "./i18n";
import "./appUpdateNotice.css";

/**
 * A quiet card for a downloaded update. It stays out of the way while an Agent turn runs, and
 * "Later" hides it until a newer version arrives.
 */
export function AppUpdateNotice({ running, api = tauriAppUpdateApi }: { running: boolean; api?: AppUpdateApi }) {
  const { t } = useI18n();
  const [update, setUpdate] = useState<AppUpdate | null>(null);
  const [later, setLater] = useState(laterVersion);
  const [installing, setInstalling] = useState(false);
  const [failure, setFailure] = useState<AppUpdateFailure | null>(null);

  useEffect(() => {
    let closed = false;
    let unlisten: (() => void) | undefined;
    const show = (next: AppUpdate) => { if (!closed) { setUpdate(next); setFailure(null); } };
    void api.onReady(show).then(stop => { if (closed) stop(); else unlisten = stop; }).catch(() => {});
    // The backend may have finished before this window started listening.
    void api.ready().then(ready => { if (ready) show(ready); }).catch(() => {});
    return () => { closed = true; unlisten?.(); };
  }, [api]);

  if (!installing && !shouldOfferUpdate(update, later, running)) return null;
  const install = () => {
    setInstalling(true); setFailure(null);
    // Success restarts the app, so only a failure comes back here.
    api.install().catch(cause => { setInstalling(false); setFailure(installFailure(cause)); });
  };
  const dismiss = () => { rememberLater(update!.version); setLater(update!.version); };
  return <section className="app-update-notice" aria-label={t("appUpdate.label")}>
    <div role="status">
      <strong>{t("appUpdate.ready", { version: update!.version })}</strong>
      <p>{t("appUpdate.detail")}</p>
    </div>
    {failure && <p role="alert">{t(failure === "busy" ? "appUpdate.busy" : "appUpdate.failed")}</p>}
    <div className="app-update-notice-actions">
      <DialogButton disabled={installing} onClick={dismiss}>{t("appUpdate.later")}</DialogButton>
      <DialogButton tone="primary" icon="check" disabled={installing} onClick={install}>
        {installing ? t("appUpdate.installing") : t("appUpdate.install")}
      </DialogButton>
    </div>
  </section>;
}
