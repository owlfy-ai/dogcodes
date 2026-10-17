//! Request facts, separate from native turn/session snapshots. Missing is not zero.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageRequest {
    /// Client-observed first output to native request completion; excludes tool execution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation_ms: Option<u64>,
    /// Stable within this native session. A native turn key is admissible only
    /// when its telemetry explicitly proves it contains exactly one model call.
    pub request_id: String,
    pub model: Option<String>,
    pub source: String,
    /// Normalized input includes cache reads and writes; output includes reasoning.
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
    pub cost_usd: Option<f64>,
    pub credits: Option<f64>,
}
impl UsageRequest {
    #[must_use]
    pub fn has_data(&self) -> bool {
        !self.request_id.is_empty()
            && (self.input_tokens.is_some()
                || self.output_tokens.is_some()
                || self.cost_usd.is_some()
                || self.credits.is_some())
            && self.cost_usd.is_none_or(|v| v.is_finite() && v >= 0.0)
            && self.credits.is_none_or(|v| v.is_finite() && v >= 0.0)
            && self
                .cache_read_tokens
                .zip(self.input_tokens)
                .is_none_or(|(c, i)| c <= i)
            && self
                .cache_write_tokens
                .zip(self.input_tokens)
                .is_none_or(|(c, i)| c <= i)
            && self
                .reasoning_tokens
                .zip(self.output_tokens)
                .is_none_or(|(r, o)| r <= o)
            && self
                .cache_read_tokens
                .zip(self.cache_write_tokens)
                .zip(self.input_tokens)
                .is_none_or(|((r, w), i)| r.checked_add(w).is_some_and(|c| c <= i))
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestUsageReport {
    /// Observed records only. This never asserts coverage of the entire session.
    pub records: Vec<UsageRequest>,
    pub truncated: bool,
    /// A provider-specific explanation when request identities/usage are unavailable.
    pub unavailable_reason: Option<String>,
}
impl RequestUsageReport {
    /// Replace repeated native observations, never add them as new requests.
    pub fn observe(&mut self, record: UsageRequest) {
        if !record.has_data() {
            return;
        }
        self.unavailable_reason = None;
        if let Some(old) = self
            .records
            .iter_mut()
            .find(|old| old.request_id == record.request_id)
        {
            *old = record;
        } else if self.records.len() < 10_000 {
            self.records.push(record);
        } else {
            self.truncated = true;
        }
    }
    #[must_use]
    pub fn unavailable(reason: &str) -> Self {
        Self {
            unavailable_reason: Some(reason.into()),
            ..Self::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn repeated_updates_replace_and_unknown_is_preserved() {
        let mut report = RequestUsageReport::default();
        let mut record = UsageRequest {
            request_id: "r".into(),
            input_tokens: Some(100),
            ..UsageRequest::default()
        };
        report.observe(record.clone());
        record.output_tokens = Some(10);
        report.observe(record);
        assert_eq!(report.records.len(), 1);
        assert_eq!(report.records[0].output_tokens, Some(10));
        assert_eq!(report.records[0].cache_read_tokens, None);
        report.observe(UsageRequest {
            input_tokens: Some(100),
            ..UsageRequest::default()
        });
        assert_eq!(report.records.len(), 1);
    }
}
