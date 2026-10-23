//! Recover a lost pairing ACK only after fresh key-pinned endpoint authentication.
use super::{
    Arc, Duration, RemoteHost, ServiceControl, Value, failure, host_name, http, http_body, json,
};
use dogcodes_endpoint::pairing::DeviceLease;
use serde::Deserialize;
impl RemoteHost {
    pub(super) async fn pairing_receipt(
        &self,
        lease: &DeviceLease,
        peer: [u8; 32],
        binding: &str,
        args: Value,
    ) -> Result<Value, String> {
        let id = args
            .as_str()
            .filter(|id| !id.is_empty() && id.len() <= 128)
            .ok_or("配对邀请标识无效")?;
        let (config, secrets, grant) = {
            let state = self.state.lock().await;
            let ready = state.ready.as_ref().ok_or("remote unavailable")?;
            let grant = ready.book.receipt_for_lease(id, lease).map_err(failure)?;
            if grant.public_key != hex::encode(peer) {
                return Err("配对身份不一致".into());
            }
            (ready.config.clone(), ready.secrets.clone(), grant)
        };
        let route = http(
            &config,
            &secrets,
            reqwest::Method::POST,
            &format!("devices/{}/route", grant.device_id),
        )
        .await?;
        {
            let state = self.state.lock().await;
            state
                .ready
                .as_ref()
                .ok_or("remote unavailable")?
                .book
                .receipt_for_lease(id, lease)
                .map_err(failure)?;
        }
        Ok(
            json!({"type":"paired","hostId":config.host_id,"hostName":host_name(),"invitationId":id,"phonePublicKey":hex::encode(peer),"hostPublicKey":config.public_key,"binding":binding,"grantRevision":grant.revision.to_string(),"routeToken":route["routeToken"]}),
        )
    }
    pub(super) fn start_revocation_sync(
        self: &Arc<Self>,
        control: std::sync::Weak<ServiceControl>,
    ) {
        if self
            .revocations_started
            .swap(true, std::sync::atomic::Ordering::AcqRel)
        {
            return;
        }
        let host = Arc::downgrade(self);
        tokio::spawn(async move {
            loop {
                if control.upgrade().is_none() {
                    return;
                }
                let Some(host) = host.upgrade() else {
                    return;
                };
                let _ = host.sync_revocations().await;
                drop(host);
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        });
    }
    async fn sync_revocations(&self) -> Result<(), String> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Cloud {
            device_id: String,
            revoked: bool,
        }
        let (config, secrets) = {
            let state = self.state.lock().await;
            let ready = state.ready.as_ref().ok_or("remote unavailable")?;
            (ready.config.clone(), ready.secrets.clone())
        };
        let cloud: Vec<Cloud> =
            serde_json::from_value(http(&config, &secrets, reqwest::Method::GET, "devices").await?)
                .map_err(failure)?;
        let (revoked, missing) = {
            let mut state = self.state.lock().await;
            let ready = state.ready.as_mut().ok_or("remote unavailable")?;
            for device in &cloud {
                if device.revoked
                    && ready
                        .book
                        .grants()
                        .iter()
                        .any(|grant| grant.device_id == device.device_id && !grant.revoked)
                {
                    ready.book.revoke(&device.device_id).map_err(failure)?;
                }
            }
            let revoked = ready
                .book
                .grants()
                .into_iter()
                .filter(|grant| {
                    grant.revoked
                        && cloud
                            .iter()
                            .any(|device| device.device_id == grant.device_id && !device.revoked)
                })
                .map(|grant| grant.device_id)
                .collect::<Vec<_>>();
            let missing = ready
                .book
                .active_receipts()
                .map_err(failure)?
                .into_iter()
                .filter(|(_, grant)| {
                    !cloud
                        .iter()
                        .any(|device| device.device_id == grant.device_id)
                })
                .collect::<Vec<_>>();
            (revoked, missing)
        };
        for device in revoked {
            http(
                &config,
                &secrets,
                reqwest::Method::DELETE,
                &format!("devices/{device}"),
            )
            .await?;
        }
        for (invitation, grant) in missing {
            // Retry only missing records. A cloud revocation is never turned
            // into a registration or silently reactivated by synchronization.
            let state = self.state.lock().await;
            let ready = state.ready.as_ref().ok_or("remote unavailable")?;
            if !ready.book.grants().iter().any(|g| g == &grant) {
                continue;
            }
            http_body(
                &config,
                &secrets,
                reqwest::Method::POST,
                &format!("devices/{}", grant.device_id),
                Some(&json!({"invitationId":invitation,"onlyIfMissing":true})),
            )
            .await?;
        }
        Ok(())
    }
}
