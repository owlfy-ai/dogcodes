//! Recover a rejected, unpaired routing registration without replacing endpoint keys.
use super::{Config, Ready, Secrets, State, Value, decode_http, http_client, http_request};
use std::{io::Write, path::Path};

pub(super) fn current_registration(state: &State, config: &Config) -> bool {
    state.ready.as_ref().is_some_and(|ready| {
        ready.config.host_id == config.host_id && ready.config.service == config.service
    })
}

pub(super) async fn register_host(
    service: &str,
    public_key: String,
    private_key: String,
) -> Result<(Config, Secrets), String> {
    let response = http_client(service)?
        .post(format!("{service}/v1/hosts"))
        .send()
        .await
        .map_err(|_| "无法连接接入服务，请确认服务已启动")?;
    let registration = decode_http(response).await?;
    Ok((
        Config {
            service: service.into(),
            host_id: registration["hostId"]
                .as_str()
                .filter(|id| !id.is_empty())
                .ok_or("缺少电脑标识")?
                .into(),
            public_key,
        },
        Secrets {
            token: registration["hostToken"]
                .as_str()
                .filter(|token| !token.is_empty())
                .ok_or("缺少路由凭证")?
                .into(),
            private_key,
        },
    ))
}

pub(super) async fn save_registration(
    directory: &Path,
    config: &Config,
    secrets: &Secrets,
) -> Result<(), String> {
    let (directory, config, secrets) = (directory.to_owned(), config.clone(), secrets.clone());
    tokio::task::spawn_blocking(move || {
        let entry = keyring::Entry::new("ai.owlfy.dogcodes.remote", &config.host_id)
            .map_err(|_| "无法访问系统密钥存储")?;
        persist_registration(&directory, &config, &secrets, &entry)
    })
    .await
    .map_err(|_| "保存系统密钥失败")?
}

// Write credentials first, then atomically publish their reference. A failed save
// leaves the old registration usable; private keys never enter the config file.
fn persist_registration(
    directory: &Path,
    config: &Config,
    secrets: &Secrets,
    entry: &keyring::Entry,
) -> Result<(), String> {
    std::fs::create_dir_all(directory).map_err(|_| "无法保存远程配置")?;
    let mut file = tempfile::NamedTempFile::new_in(directory).map_err(|_| "无法保存远程配置")?;
    let bytes = serde_json::to_vec(config).map_err(|_| "无法保存远程配置")?;
    file.write_all(&bytes).map_err(|_| "无法保存远程配置")?;
    file.as_file().sync_all().map_err(|_| "无法保存远程配置")?;
    entry
        .set_password(&serde_json::to_string(secrets).map_err(|_| "无法保存电脑身份")?)
        .map_err(|_| "系统密钥存储不可用，尚未更新电脑登记")?;
    file.persist(directory.join("config.json"))
        .map_err(|_| "无法保存远程配置")?;
    Ok(())
}

impl Ready {
    pub(super) async fn invitation(
        &mut self,
        directory: &Path,
        active_sessions: bool,
    ) -> Result<Value, String> {
        let response = http_request(
            &self.config,
            &self.secrets,
            reqwest::Method::POST,
            "invitations",
            None,
        )
        .await?;
        if response.status() != reqwest::StatusCode::UNAUTHORIZED {
            return decode_http(response).await;
        }
        // A definite 401 precedes invitation creation on the server. Only this
        // explicit local pairing action may renew an unpaired registration.
        // Never erase existing trust, rotate endpoint keys, or retry an uncertain request.
        if self.book.grants().iter().any(|grant| !grant.revoked) {
            return Err("电脑登记凭证已失效，请在本机撤销原手机绑定后重新生成配对二维码".into());
        }
        if active_sessions {
            return Err("电脑登记凭证已失效，请等待远程连接结束后重新生成配对二维码".into());
        }
        let (config, secrets) = register_host(
            &self.config.service,
            self.config.public_key.clone(),
            self.secrets.private_key.clone(),
        )
        .await?;
        save_registration(directory, &config, &secrets).await?;
        self.config = config;
        self.secrets = secrets;
        decode_http(
            http_request(
                &self.config,
                &self.secrets,
                reqwest::Method::POST,
                "invitations",
                None,
            )
            .await?,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dogcodes_endpoint::pairing::PairingBook;
    use serde_json::json;
    use sha2::{Digest, Sha256};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn server(
        responses: Vec<(u16, Value)>,
    ) -> (String, tokio::task::JoinHandle<Vec<String>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let service = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let mut paths = vec![];
            for (code, body) in responses {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = vec![];
                while !bytes.ends_with(b"\r\n\r\n") {
                    bytes.push(socket.read_u8().await.unwrap());
                }
                paths.push(
                    String::from_utf8(bytes)
                        .unwrap()
                        .lines()
                        .next()
                        .unwrap()
                        .to_owned(),
                );
                let body = body.to_string();
                socket.write_all(format!("HTTP/1.1 {code} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
            }
            paths
        });
        (service, task)
    }
    fn ready(directory: &Path, service: String, paired: bool) -> Ready {
        let path = directory.join("devices.json");
        if paired {
            let key = [7; 32];
            let id = hex::encode(Sha256::digest(key));
            std::fs::write(
                &path,
                json!({"version":1,"revision":1,"receipts":{},"grants":{
                    &id: {"deviceId":id,"publicKey":hex::encode(key),"revision":1,"revoked":false}
                }})
                .to_string(),
            )
            .unwrap();
        }
        Ready {
            config: Config {
                service,
                host_id: "old-host".into(),
                public_key: "pinned-public".into(),
            },
            secrets: Secrets {
                token: "old-token".into(),
                private_key: "pinned-private".into(),
            },
            book: PairingBook::load(path).unwrap(),
        }
    }

    #[tokio::test]
    async fn definite_unauthorized_unpaired_invitation_attempts_registration() {
        let directory = tempfile::tempdir().unwrap();
        let (service, requests) = server(vec![(401, json!({})), (503, json!({}))]).await;
        let mut ready = ready(directory.path(), service, false);
        let error = ready.invitation(directory.path(), false).await.unwrap_err();
        assert!(error.contains("503"));
        assert_eq!(
            requests.await.unwrap(),
            [
                "POST /v1/hosts/old-host/invitations HTTP/1.1",
                "POST /v1/hosts HTTP/1.1"
            ]
        );
        assert_eq!(ready.config.host_id, "old-host");
        assert_eq!(ready.secrets.private_key, "pinned-private");
        assert!(!directory.path().join("config.json").exists());
    }
    #[tokio::test]
    async fn paired_or_active_connections_never_silently_replace_registration() {
        for (paired, active) in [(true, false), (false, true)] {
            let directory = tempfile::tempdir().unwrap();
            let (service, requests) = server(vec![(401, json!({}))]).await;
            let mut ready = ready(directory.path(), service, paired);
            let before = ready.book.grants();
            assert!(
                ready
                    .invitation(directory.path(), active)
                    .await
                    .unwrap_err()
                    .contains("凭证已失效")
            );
            assert_eq!(requests.await.unwrap().len(), 1);
            assert_eq!(ready.config.host_id, "old-host");
            assert_eq!(ready.book.grants(), before);
        }
    }
    #[tokio::test]
    async fn service_failures_do_not_trigger_registration_or_retry() {
        for status in [403, 429, 500, 503] {
            let directory = tempfile::tempdir().unwrap();
            let (service, requests) = server(vec![(status, json!({}))]).await;
            let mut ready = ready(directory.path(), service, false);
            assert!(
                ready
                    .invitation(directory.path(), false)
                    .await
                    .unwrap_err()
                    .contains(&status.to_string())
            );
            assert_eq!(requests.await.unwrap().len(), 1);
            assert_eq!(ready.config.host_id, "old-host");
        }
    }
    #[tokio::test]
    async fn registration_retains_pinned_endpoint_keys() {
        let (service, requests) = server(vec![(
            201,
            json!({"hostId":"new-host","hostToken":"new-token"}),
        )])
        .await;
        let (config, secrets) =
            register_host(&service, "pinned-public".into(), "pinned-private".into())
                .await
                .unwrap();
        assert_eq!(config.host_id, "new-host");
        assert_eq!(config.public_key, "pinned-public");
        assert_eq!(secrets.private_key, "pinned-private");
        assert_eq!(secrets.token, "new-token");
        assert_eq!(requests.await.unwrap(), ["POST /v1/hosts HTTP/1.1"]);
    }
    #[test]
    fn keychain_failure_keeps_old_config_and_secrets_out_of_config() {
        let directory = tempfile::tempdir().unwrap();
        let ready = ready(directory.path(), "http://127.0.0.1:1".into(), false);
        let path = directory.path().join("config.json");
        std::fs::write(&path, b"original").unwrap();
        let credential = keyring::mock::MockCredential::default();
        credential.set_error(keyring::Error::NoEntry);
        let entry = keyring::Entry::new_with_credential(Box::new(credential));
        assert!(
            persist_registration(directory.path(), &ready.config, &ready.secrets, &entry).is_err()
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"original");
        persist_registration(directory.path(), &ready.config, &ready.secrets, &entry).unwrap();
        let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved["hostId"], "old-host");
        assert!(saved.get("privateKey").is_none());
        assert!(saved.get("token").is_none());
        let secret: Secrets = serde_json::from_str(&entry.get_password().unwrap()).unwrap();
        assert_eq!(secret.private_key, "pinned-private");
    }
}
