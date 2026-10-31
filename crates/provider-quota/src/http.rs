use super::{DriverError, Kind, Request, invalid};
use reqwest::{Client, StatusCode};
use serde_json::Value;
use std::time::Duration;

pub(super) fn client() -> Result<Client, DriverError> {
    Client::builder()
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent("dogcodes/provider-quota")
        .build()
        .map_err(|_| DriverError::Disconnected("provider quota HTTP unavailable".into()))
}

pub(super) async fn get(client: &Client, request: &Request) -> Result<Value, DriverError> {
    let builder = client.get(&request.endpoint);
    let builder = match request.kind {
        Kind::DeepSeekPlatform => builder.header("x-dsh-auth-token", &request.key),
        _ => builder.bearer_auth(&request.key),
    };
    let mut response = builder
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|_| {
            DriverError::Disconnected("provider quota request failed or timed out".into())
        })?;
    match response.status() {
        StatusCode::UNAUTHORIZED => {
            return Err(DriverError::NotConfigured("authentication_failed".into()));
        }
        StatusCode::FORBIDDEN => {
            return Err(DriverError::NotConfigured(
                if matches!(request.kind, Kind::Go) {
                    "subscription_required"
                } else {
                    "access_denied"
                }
                .into(),
            ));
        }
        StatusCode::TOO_MANY_REQUESTS => {
            return Err(DriverError::Protocol("agent_rate_limited".into()));
        }
        status if !status.is_success() => {
            return Err(DriverError::Disconnected(
                "provider quota request unsuccessful".into(),
            ));
        }
        _ => {}
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| invalid())? {
        if body.len() + chunk.len() > 1024 * 1024 {
            return Err(invalid());
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|_| invalid())
}
