import { useEffect, useState, useSyncExternalStore } from "react";
import { invoke } from "@tauri-apps/api/core";
import { AppDialog, DialogButton } from "./AppDialog";
import { gitOperations } from "./gitOperations";
import { useI18n } from "./i18n";
import { userFacingError } from "./userFacingError";

interface CredentialPrompt { id: number; text: string; secret: boolean }

export interface GitCredentialApi {
  current(taskId: string): Promise<CredentialPrompt | null>;
  answer(taskId: string, id: number, answer: string | null): Promise<void>;
}

const tauriApi: GitCredentialApi = {
  current: taskId => invoke("core_review_git_credential", { taskId }),
  answer: (taskId, id, answer) => invoke("core_review_git_credential_answer", { taskId, id, answer }),
};

const POLL_MILLISECONDS = 400;

/** Git can only be waiting on an answer while one of its operations is running here. */
export function GitCredentialPrompt({ taskId, api = tauriApi }: { taskId: string; api?: GitCredentialApi }) {
  const operations = useSyncExternalStore(gitOperations.subscribe, () => gitOperations.snapshot(taskId), () => "[]");
  const running = operations.includes('"sending":true');
  const [prompt, setPrompt] = useState<CredentialPrompt | null>(null);
  useEffect(() => {
    if (!running) { setPrompt(null); return; }
    let closed = false;
    const read = () => void api.current(taskId).then(value => { if (!closed) setPrompt(value); }).catch(() => {});
    read();
    const timer = setInterval(read, POLL_MILLISECONDS);
    return () => { closed = true; clearInterval(timer); };
  }, [api, taskId, running]);
  if (!prompt) return null;
  return <CredentialDialog key={prompt.id} prompt={prompt} onAnswer={answer => api.answer(taskId, prompt.id, answer).then(() => setPrompt(null))} />;
}

function CredentialDialog({ prompt, onAnswer }: { prompt: CredentialPrompt; onAnswer: (answer: string | null) => Promise<void> }) {
  const { t } = useI18n();
  const [value, setValue] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const send = (answer: string | null) => {
    setBusy(true); setError("");
    void onAnswer(answer).catch(cause => { setBusy(false); setError(userFacingError(cause, t("gitCredential.failed"))); });
  };
  return <AppDialog title={t("gitCredential.title")} onSubmit={() => send(value)} onCancel={() => send(null)} actions={<>
    <DialogButton icon="cancel" disabled={busy} onClick={() => send(null)}>{t("common.cancel")}</DialogButton>
    <DialogButton type="submit" tone="primary" icon="check" disabled={busy}>{t("gitCredential.submit")}</DialogButton>
  </>}>
    <label>{prompt.text}
      <input data-dialog-default type={prompt.secret ? "password" : "text"} autoComplete="off" autoCapitalize="off" spellCheck={false} value={value} onChange={event => setValue(event.target.value)} />
    </label>
    <p>{t("gitCredential.hint")}</p>
    {error && <p role="alert">{error}</p>}
  </AppDialog>;
}
