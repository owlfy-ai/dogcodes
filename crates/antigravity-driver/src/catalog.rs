//! Metadata queries also retain their original OS scope and shared auth exclusion.
use super::{Child, DriverError, ENV_ALLOWLIST, PROBE_TIMEOUT, PathBuf, SpawnSlot, Stdio};
use tokio::io::AsyncReadExt;

pub(super) async fn read(
    executable: PathBuf,
    slot: SpawnSlot,
) -> Result<Vec<(String, String)>, DriverError> {
    let (_observer, mut cancelled) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(async move {
        let abandoned = || DriverError::Disconnected("model catalog caller left".into());
        if cancelled.try_recv() == Err(tokio::sync::oneshot::error::TryRecvError::Closed) {
            return Err(abandoned());
        }
        let mut command = tokio::process::Command::new(executable);
        command
            .arg("models")
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        for key in ENV_ALLOWLIST {
            if let Some(value) = agent_runtime::inherited_variable(key) {
                command.env(key, value);
            }
        }
        agent_runtime::apply_proxy_environment(&mut command);
        let fail = || DriverError::Protocol("native model catalog unavailable".into());
        let mut child = Child::spawn(command).map_err(|_| fail())?;
        let result = tokio::select! {
            biased;
            _ = &mut cancelled => Err(abandoned()),
            result = tokio::time::timeout(PROBE_TIMEOUT, async {
                let mut bytes = Vec::new();
                child
                    .stdout
                    .take()
                    .ok_or_else(fail)?
                    .take(65537)
                    .read_to_end(&mut bytes)
                    .await
                    .map_err(|_| fail())?;
                if bytes.len() > 65536 || !child.wait().await.map_err(|_| fail())?.success() {
                    return Err(fail());
                }
                Ok(bytes)
            }) => result.map_err(|_| DriverError::Disconnected("model catalog timed out".into())),
        };
        let mut delay = 1;
        while child.terminate().await.is_err() {
            tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
            delay = (delay * 2).min(30);
        }
        drop(slot);
        let bytes = result??;
        let models: Vec<(String, String)> = String::from_utf8_lossy(&bytes)
            .lines()
            .filter_map(|line| {
                line.split_once('\t')
                    .map(|(id, label)| (id.to_owned(), label.trim().to_owned()))
            })
            .filter(|(id, _)| {
                !id.is_empty()
                    && id.len() <= 200
                    && id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
            })
            .collect();
        if models.is_empty() {
            return Err(fail());
        }
        Ok(models)
    })
    .await
    .map_err(|_| DriverError::Disconnected("model catalog worker ended without a result".into()))?
}
