//! Read-only monitoring of the accounts behind `OpenCode`, Pi and `DeepSeek` Harness, on the
//! machine they are signed in on. Credentials stay inside this module: what leaves is each
//! provider's answer, for the asker to make sense of, and a hash to tell accounts apart.
mod credentials;
#[cfg(test)]
mod tests;

use crate::{http, operation::Failure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{collections::HashMap, future::Future};

pub type Environment = HashMap<String, String>;

const USER_AGENT: &str = "User-Agent: dogcodes/provider-quota";
const BODY_LIMIT: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Go,
    DeepSeek,
    DeepSeekPlatform,
    Sub2Api,
}

// Deliberately no Debug/Serialize: it contains native credentials.
#[derive(Clone)]
pub struct Credential {
    pub kind: Kind,
    pub label: String,
    pub endpoint: String,
    pub key: String,
    /// `OpenCode` Go subscriptions are scoped to the signed-in organization.
    pub org_id: Option<String>,
}

/// An installed agent whose account is being read.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Agent {
    pub provider: String,
    pub executable: String,
    pub version: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Reading {
    pub kind: Kind,
    pub label: String,
    pub outcome: Result<Value, Failure>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Usage {
    /// The account is still the one the asker named, so nobody was contacted.
    Unchanged,
    Read {
        fingerprint: String,
        readings: Vec<Reading>,
    },
}

fn fingerprint(credentials: &[Credential]) -> String {
    let mut digest = Sha256::new();
    for credential in credentials {
        for part in [
            credential.label.as_str(),
            credential.endpoint.as_str(),
            credential.key.as_str(),
            credential.org_id.as_deref().unwrap_or(""),
        ] {
            digest.update(part.len().to_le_bytes());
            digest.update(part.as_bytes());
        }
    }
    hex::encode(digest.finalize())
}

/// Failures are per account, as one provider being down says nothing about another.
///
/// # Errors
/// When there are no credentials to use, or they changed while reading: an answer that may
/// belong to the previous account is never passed on.
pub async fn read<Answer: Future<Output = Result<Value, Failure>>>(
    agent: &Agent,
    environment: &Environment,
    known: Option<&str>,
    get: impl Fn(Credential) -> Answer,
) -> Result<Usage, Failure> {
    let credentials = credentials::discover(agent, environment).await?;
    let current = fingerprint(&credentials);
    if known == Some(current.as_str()) {
        return Ok(Usage::Unchanged);
    }
    let get = &get;
    let readings = futures::future::join_all(credentials.into_iter().map(|c| async move {
        let (kind, label) = (c.kind, c.label.clone());
        Reading {
            kind,
            label,
            outcome: get(c).await,
        }
    }))
    .await;
    if fingerprint(&credentials::discover(agent, environment).await?) != current {
        return Err(credentials::unavailable(
            "provider credentials changed during refresh",
        ));
    }
    Ok(Usage::Read {
        fingerprint: current,
        readings,
    })
}

/// What a response's status means, whichever client received it.
#[must_use]
pub fn status_failure(kind: Kind, status: u16) -> Option<Failure> {
    match status {
        200..=299 => None,
        401 => Some(Failure::new("login", "authentication_failed")),
        403 => Some(Failure::new(
            "login",
            if kind == Kind::Go {
                "subscription_required"
            } else {
                "access_denied"
            },
        )),
        429 => Some(Failure::new("protocol", "agent_rate_limited")),
        _ => Some(Failure::new(
            "disconnected",
            "provider quota request unsuccessful",
        )),
    }
}

#[must_use]
pub fn invalid_response() -> Failure {
    Failure::new("protocol", "provider quota response invalid")
}

/// Redirects stay off: a key goes to its configured origin and nowhere else.
async fn over_curl(credential: Credential) -> Result<Value, Failure> {
    let lost = || Failure::new("disconnected", "provider quota request failed or timed out");
    let authentication = if credential.kind == Kind::DeepSeekPlatform {
        format!("x-dsh-auth-token: {}", credential.key)
    } else {
        format!("Authorization: Bearer {}", credential.key)
    };
    let organization = credential
        .org_id
        .as_ref()
        .filter(|_| credential.kind == Kind::Go)
        .map(|id| format!("x-opencode-org-id: {id}"));
    let mut headers = vec![
        authentication.as_str(),
        USER_AGENT,
        "Accept: application/json",
    ];
    headers.extend(organization.as_deref());
    let output = http::curl(
        None,
        &headers,
        &[
            "--proto",
            "=http,https",
            "--write-out",
            "\n%{http_code}",
            &credential.endpoint,
        ],
    )
    .await
    .map_err(|_| lost())?;
    let split = output
        .stdout
        .iter()
        .rposition(|byte| *byte == b'\n')
        .ok_or_else(lost)?;
    let status = std::str::from_utf8(&output.stdout[split + 1..])
        .ok()
        .and_then(|status| status.parse().ok())
        .ok_or_else(lost)?;
    if let Some(failure) = status_failure(credential.kind, status) {
        return Err(failure);
    }
    if split > BODY_LIMIT {
        return Err(invalid_response());
    }
    serde_json::from_slice(&output.stdout[..split]).map_err(|_| invalid_response())
}

/// The shim's side. It has no TLS of its own, so it asks the server's `curl`, which also
/// goes out the way everything else on that server does.
///
/// # Errors
/// As [`read`].
pub async fn read_through_curl(agent: &Agent, known: Option<&str>) -> Result<Usage, Failure> {
    let environment = std::env::vars_os()
        .filter_map(|(name, value)| Some((name.into_string().ok()?, value.into_string().ok()?)))
        .collect();
    read(agent, &environment, known, over_curl).await
}
