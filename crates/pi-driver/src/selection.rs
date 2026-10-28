//! A failed multi-command selection must not leave a warm session half changed.
use super::{Connection, DriverError, Value, json, model_id};

struct Attempt<'a> {
    connection: &'a Connection,
    confirmed: bool,
}
impl Drop for Attempt<'_> {
    fn drop(&mut self) {
        if !self.confirmed {
            self.connection.request_close();
        }
    }
}

async fn apply(
    connection: &Connection,
    model: Option<&str>,
    thinking: Option<&str>,
) -> Result<Value, DriverError> {
    if let Some(model) = model {
        let (provider, id) = model
            .split_once('/')
            .filter(|(p, m)| !p.is_empty() && !m.is_empty())
            .ok_or_else(|| {
                DriverError::CapabilityUnavailable("Invalid Pi model selection".into())
            })?;
        connection
            .call(json!({"type":"set_model","provider":provider,"modelId":id}))
            .await?;
    }
    if let Some(level) = thinking {
        let levels = connection
            .call(json!({"type":"get_available_thinking_levels"}))
            .await?;
        if !levels["levels"]
            .as_array()
            .is_some_and(|levels| levels.iter().any(|v| v == level))
        {
            return Err(DriverError::CapabilityUnavailable(
                "Pi model does not support this thinking level".into(),
            ));
        }
        connection
            .call(json!({"type":"set_thinking_level","level":level}))
            .await?;
    }
    let state = connection.call(json!({"type":"get_state"})).await?;
    if state["isStreaming"] != false {
        return Err(DriverError::SessionIncompatible(
            "Pi native session is not idle".into(),
        ));
    }
    if model.is_some_and(|selected| model_id(&state["model"]).as_deref() != Some(selected))
        || thinking.is_some_and(|selected| state["thinkingLevel"] != selected)
    {
        return Err(DriverError::Protocol(
            "Pi did not confirm model/thinking selection".into(),
        ));
    }
    Ok(state)
}

pub(super) async fn configure(
    connection: &Connection,
    model: Option<&str>,
    thinking: Option<&str>,
) -> Result<Value, DriverError> {
    let before = connection.call(json!({"type":"get_state"})).await?;
    if before["isStreaming"] != false {
        connection.request_close();
        return Err(DriverError::SessionIncompatible(
            "Pi native session is not idle".into(),
        ));
    }
    if model.is_none() && thinking.is_none() {
        return Ok(before);
    }
    let mut attempt = Attempt {
        connection,
        confirmed: false,
    };
    match apply(connection, model, thinking).await {
        Ok(after) if after["sessionFile"] == before["sessionFile"] => {
            attempt.confirmed = true;
            return Ok(after);
        }
        Ok(_) => {
            return Err(DriverError::SessionIncompatible(
                "Pi session changed during configuration".into(),
            ));
        }
        Err(error) if connection.state.lock().expect("Pi state").disconnected => return Err(error),
        Err(error @ DriverError::SessionIncompatible(_)) => return Err(error),
        Err(_) => {}
    }
    let prior_model = model_id(&before["model"]);
    let prior_thinking = before["thinkingLevel"].as_str();
    if let (Some(prior_model), Some(prior_thinking)) = (prior_model, prior_thinking)
        && let Ok(after) = apply(connection, Some(&prior_model), Some(prior_thinking)).await
        && after["sessionFile"] == before["sessionFile"]
    {
        attempt.confirmed = true;
        return Err(DriverError::Protocol("selection_rejected".into()));
    }
    Err(DriverError::Protocol(
        "Pi selection remains unconfirmed".into(),
    ))
}
