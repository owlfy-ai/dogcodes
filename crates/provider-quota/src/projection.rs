use super::{DriverError, invalid};
use driver_api::{ProviderBalance, QuotaWindow};
use serde_json::Value;

pub(super) fn go(payload: &Value) -> Result<Vec<QuotaWindow>, DriverError> {
    let usage = payload["usage"].as_object().ok_or_else(invalid)?;
    let mut windows = Vec::new();
    for (name, minutes) in [("rolling", 300), ("weekly", 10_080), ("monthly", 0)] {
        let window = usage.get(name).ok_or_else(invalid)?;
        if !matches!(window["status"].as_str(), Some("ok" | "rate-limited")) {
            return Err(invalid());
        }
        let percent = window["percent"]
            .as_f64()
            .filter(|v| v.is_finite() && *v >= 0.0)
            .ok_or_else(invalid)?;
        let reset =
            chrono::DateTime::parse_from_rfc3339(window["resetsAt"].as_str().ok_or_else(invalid)?)
                .map_err(|_| invalid())?
                .timestamp();
        windows.push(QuotaWindow {
            // Informational: OpenCode can use other providers and Go can fall back to credits.
            scope: Some("OpenCode Go".into()),
            id: format!("opencode-go-{name}"),
            window_minutes: minutes,
            used_percent: Some(percent.min(100.0)),
            resets_at_unix: Some(reset),
        });
    }
    Ok(windows)
}

fn amount(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str()?.parse().ok())
        .filter(|n| n.is_finite() && *n >= 0.0)
}
fn balance(label: &str, value: &Value, currency: &str) -> Option<ProviderBalance> {
    if currency.len() != 3 || !currency.bytes().all(|c| c.is_ascii_uppercase()) {
        return None;
    }
    Some(ProviderBalance {
        label: label.into(),
        amount: amount(value)?,
        currency: currency.into(),
    })
}
pub(super) fn deepseek(payload: &Value) -> Result<Vec<ProviderBalance>, DriverError> {
    let rows = payload["balance_infos"].as_array().ok_or_else(invalid)?;
    let balances: Vec<_> = rows
        .iter()
        .filter_map(|row| {
            balance(
                "DeepSeek API",
                &row["total_balance"],
                row["currency"].as_str()?,
            )
        })
        .collect();
    if balances.is_empty() {
        return Err(invalid());
    }
    Ok(balances)
}
pub(super) fn sub2api(payload: &Value, label: &str) -> Result<Vec<ProviderBalance>, DriverError> {
    if payload["isValid"] == false
        || !matches!(
            payload["mode"].as_str(),
            Some("unrestricted" | "quota_limited")
        )
    {
        return Err(invalid());
    }
    let currency = payload["unit"].as_str().ok_or_else(invalid)?;
    let value = [
        &payload["balance"],
        &payload["remaining"],
        &payload["quota"]["remaining"],
    ]
    .into_iter()
    .find_map(|value| balance(label, value, currency))
    .ok_or_else(invalid)?;
    Ok(vec![value])
}
