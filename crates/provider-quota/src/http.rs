use agent_management::{
    operation::Failure,
    provider_account::{Credential, Kind, invalid_response, status_failure},
};
use reqwest::Client;
use serde_json::Value;
use std::time::Duration;

fn client() -> Result<Client, Failure> {
    Client::builder()
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent("dogcodes/provider-quota")
        .build()
        .map_err(|_| Failure::new("disconnected", "provider quota HTTP unavailable"))
}

pub(super) async fn get(credential: &Credential) -> Result<Value, Failure> {
    let builder = client()?.get(&credential.endpoint);
    let builder = match credential.kind {
        Kind::DeepSeekPlatform => builder.header("x-dsh-auth-token", &credential.key),
        _ => builder.bearer_auth(&credential.key),
    };
    let builder = match &credential.org_id {
        Some(org_id) if credential.kind == Kind::Go => builder.header("x-opencode-org-id", org_id),
        _ => builder,
    };
    let mut response = builder
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|_| Failure::new("disconnected", "provider quota request failed or timed out"))?;
    if let Some(failure) = status_failure(credential.kind, response.status().as_u16()) {
        return Err(failure);
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| invalid_response())? {
        if body.len() + chunk.len() > 1024 * 1024 {
            return Err(invalid_response());
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|_| invalid_response())
}
