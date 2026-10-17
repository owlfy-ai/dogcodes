//! Deterministic, local-only secret and identity redaction.

use std::{collections::BTreeMap, sync::LazyLock};

use regex::{Captures, Regex};

pub mod structured;

pub const REDACTION_RULESET_VERSION: u16 = 2;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RedactedText {
    pub text: String,
    pub finding_counts: BTreeMap<&'static str, u32>,
}

impl RedactedText {
    #[must_use]
    pub fn finding_count(&self) -> u32 {
        self.finding_counts
            .values()
            .fold(0_u32, |total, count| total.saturating_add(*count))
    }
}

pub trait SecretRedactor: Send + Sync {
    fn redact(&self, input: &str) -> RedactedText;

    fn ruleset_version(&self) -> u16;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct DeterministicRedactor;

impl SecretRedactor for DeterministicRedactor {
    fn redact(&self, input: &str) -> RedactedText {
        let mut text = input.to_owned();
        let mut finding_counts = BTreeMap::new();

        apply_with(
            &mut text,
            "quoted_secret",
            &QUOTED_SECRET,
            |captures| format!("{}\"[REDACTED]\"", &captures[1]),
            &mut finding_counts,
        );
        apply_with(
            &mut text,
            "secret_assignment",
            &SECRET_ASSIGNMENT,
            |captures| {
                let value = &captures[2];
                let replacement = if value.starts_with('"') {
                    "\"[REDACTED]\""
                } else if value.starts_with('\'') {
                    "'[REDACTED]'"
                } else {
                    "[REDACTED]"
                };
                format!("{}{replacement}", &captures[1])
            },
            &mut finding_counts,
        );
        apply(
            &mut text,
            "private_key",
            &PRIVATE_KEY,
            "[REDACTED_PRIVATE_KEY]",
            &mut finding_counts,
        );
        apply(
            &mut text,
            "known_token",
            &KNOWN_TOKEN,
            "[REDACTED_TOKEN]",
            &mut finding_counts,
        );
        apply_with(
            &mut text,
            "authorization",
            &AUTHORIZATION,
            |captures| format!("{} [REDACTED]", &captures[1]),
            &mut finding_counts,
        );
        apply_with(
            &mut text,
            "authorization",
            &BASIC_AUTHORIZATION,
            |captures| format!("{}[REDACTED]", &captures[1]),
            &mut finding_counts,
        );
        apply_with(
            &mut text,
            "url_userinfo",
            &URL_USERINFO,
            |captures| format!("{}[REDACTED_USERINFO]@", &captures[1]),
            &mut finding_counts,
        );
        apply_with(
            &mut text,
            "high_entropy",
            &ENTROPY_CANDIDATE,
            |captures| {
                if high_entropy_candidate(&captures[0]) {
                    "[REDACTED_HIGH_ENTROPY]".into()
                } else {
                    captures[0].to_owned()
                }
            },
            &mut finding_counts,
        );
        apply(
            &mut text,
            "email",
            &EMAIL,
            "[REDACTED_EMAIL]",
            &mut finding_counts,
        );
        apply_with(
            &mut text,
            "home_path",
            &UNIX_HOME,
            |captures| format!("~{}", captures.get(1).map_or("", |value| value.as_str())),
            &mut finding_counts,
        );
        apply_with(
            &mut text,
            "home_path",
            &WINDOWS_HOME,
            |captures| format!("~{}", captures.get(1).map_or("", |value| value.as_str())),
            &mut finding_counts,
        );

        RedactedText {
            text,
            finding_counts,
        }
    }

    fn ruleset_version(&self) -> u16 {
        REDACTION_RULESET_VERSION
    }
}

fn apply(
    text: &mut String,
    kind: &'static str,
    pattern: &Regex,
    replacement: &str,
    counts: &mut BTreeMap<&'static str, u32>,
) {
    apply_with(text, kind, pattern, |_| replacement.to_owned(), counts);
}

fn apply_with(
    text: &mut String,
    kind: &'static str,
    pattern: &Regex,
    replacement: impl Fn(&Captures<'_>) -> String,
    counts: &mut BTreeMap<&'static str, u32>,
) {
    let mut count = 0_u32;
    let output = pattern.replace_all(text, |captures: &Captures<'_>| {
        let value = replacement(captures);
        if value != captures[0] {
            count = count.saturating_add(1);
        }
        value
    });
    if count > 0 {
        *text = output.into_owned();
        let current = counts.entry(kind).or_default();
        *current = current.saturating_add(count);
    }
}

static AUTHORIZATION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(Bearer)\s+[A-Za-z0-9._~+/=-]+").expect("authorization regex is valid")
});
static BASIC_AUTHORIZATION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b((?:proxy-)?authorization\s*:\s*basic\s+)[A-Za-z0-9+/=]+")
        .expect("basic authorization regex is valid")
});
static URL_USERINFO: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"([A-Za-z][A-Za-z0-9+.-]*://)[^/\s?#]*@").expect("URL userinfo regex is valid")
});
static PRIVATE_KEY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)-----BEGIN (?:[A-Z0-9 ]+ )?PRIVATE KEY-----.*?(?:-----END (?:[A-Z0-9 ]+ )?PRIVATE KEY-----|$)")
        .expect("private key regex is valid")
});
static QUOTED_SECRET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)("(?:api[_-]?key|access[_-]?token|refresh[_-]?token|id[_-]?token|client[_-]?secret|private[_-]?key|password|passwd|secret|token|authorization|proxy-authorization|cookie|set-cookie)"\s*:\s*)"(?:\\.|[^"\\])*""#)
        .expect("quoted secret regex is valid")
});
static KNOWN_TOKEN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\b(?:sk-[A-Za-z0-9_-]{8,}|gh[pousr]_[A-Za-z0-9]{8,}|github_pat_[A-Za-z0-9_]{8,}|xox[baprs]-[A-Za-z0-9-]{8,}|AKIA[A-Z0-9]{12,})\b",
    )
    .expect("known token regex is valid")
});
static SECRET_ASSIGNMENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)\b((?:api[_-]?key|access[_-]?token|refresh[_-]?token|id[_-]?token|client[_-]?secret|password|passwd|secret)\s*[:=]\s*)("(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'|[^\s,;}]+)"#,
    )
    .expect("secret assignment regex is valid")
});
static ENTROPY_CANDIDATE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[A-Za-z0-9_+/=-]{32,}").expect("entropy candidate regex is valid")
});

/// Integer-only collision entropy (Renyi H2) avoids platform-dependent log/float thresholds.
/// This is a candidate heuristic, not credential validation or a guarantee of secrecy.
fn high_entropy_candidate(value: &str) -> bool {
    let bytes = value.as_bytes();
    let mut counts = [0_u128; 128];
    let mut lower = false;
    let mut upper = false;
    let mut digit = false;
    for byte in bytes {
        counts[usize::from(*byte)] += 1;
        lower |= byte.is_ascii_lowercase();
        upper |= byte.is_ascii_uppercase();
        digit |= byte.is_ascii_digit();
    }
    if !((lower || upper) && digit || lower && upper) {
        return false;
    }
    let collision_sum: u128 = counts.iter().map(|count| count * count).sum();
    let length = bytes.len() as u128;
    let factor = if bytes.iter().all(u8::is_ascii_hexdigit) {
        8
    } else {
        16
    };
    factor * collision_sum <= length * length
}
static EMAIL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b[A-Za-z0-9.!#$%&'*+/=?^_`{|}~-]+@[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)+\b")
        .expect("email regex is valid")
});
static UNIX_HOME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"/Users/[^/\s]+([^\s]*)").expect("unix home regex is valid"));
static WINDOWS_HOME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)[A-Z]:\\Users\\[^\\\s]+([^\s]*)").expect("windows home regex is valid")
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_known_credentials_before_they_leave_core() {
        let input = "Authorization: Bearer abcdefghijklmnop api_key=supersecret \
                     ghp_abcdefghijklmnopqrstuvwxyz user@example.com";
        let result = DeterministicRedactor.redact(input);

        assert!(!result.text.contains("abcdefghijklmnop"));
        assert!(!result.text.contains("supersecret"));
        assert!(!result.text.contains("ghp_abcdefghijklmnopqrstuvwxyz"));
        assert!(!result.text.contains("user@example.com"));
        assert_eq!(result.finding_count(), 4);
    }

    #[test]
    fn short_words_and_relative_project_paths_are_not_over_redacted() {
        let input = "token budget and basic integration tests in crates/core/src/lib.rs";
        let result = DeterministicRedactor.redact(input);
        assert_eq!(result.text, input);
        assert_eq!(result.finding_count(), 0);
    }

    #[test]
    fn masks_quoted_json_secrets_without_breaking_json_syntax() {
        let input = r#"{"nested":{"api_key":"fake\\value\"escaped","refreshToken":"tiny","cookie":"sid=not-real"},"ok":true}"#;
        let result = DeterministicRedactor.redact(input);
        let value: serde_json::Value = serde_json::from_str(&result.text).unwrap();
        assert_eq!(value["nested"]["api_key"], "[REDACTED]");
        assert_eq!(value["nested"]["refreshToken"], "[REDACTED]");
        assert_eq!(value["nested"]["cookie"], "[REDACTED]");
        assert_eq!(value["ok"], true);
        assert_eq!(result.finding_count(), 3);
    }

    #[test]
    fn strips_basic_auth_url_userinfo_and_private_key_blocks() {
        let input = "Authorization: Basic ZmFrZTpmYWtl https://fake:p%40ss@example.invalid/path \
            -----BEGIN RSA PRIVATE KEY-----\nsynthetic key body\n-----END RSA PRIVATE KEY-----\nkeep";
        let result = DeterministicRedactor.redact(input);
        assert!(!result.text.contains("ZmFrZTpmYWtl"));
        assert!(!result.text.contains("p%40ss"));
        assert!(!result.text.contains("synthetic key body"));
        assert!(
            result
                .text
                .contains("https://[REDACTED_USERINFO]@example.invalid/path")
        );
        assert!(result.text.ends_with("keep"));
        assert_eq!(result.finding_count(), 3);
        let truncated =
            DeterministicRedactor.redact("-----BEGIN PRIVATE KEY-----\nsynthetic incomplete key");
        assert_eq!(truncated.text, "[REDACTED_PRIVATE_KEY]");
    }

    #[test]
    fn detects_unlabelled_mixed_and_hex_entropy_candidates() {
        let mixed = "Ab3dEf6hIj9lMn2pQr5tUv8xYz1BC4DE";
        let hex = "0123456789abcdef0123456789abcdef";
        let result =
            DeterministicRedactor.redact(&format!("candidate {mixed}, digest-or-secret {hex}"));
        assert_eq!(result.finding_counts.get("high_entropy"), Some(&2));
        assert!(!result.text.contains(mixed));
        assert!(!result.text.contains(hex));
        assert_eq!(
            DeterministicRedactor
                .redact("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa1111")
                .finding_count(),
            0
        );
        assert_eq!(
            DeterministicRedactor
                .redact("1234567890123456789012345678901234")
                .finding_count(),
            0
        );
    }

    #[test]
    fn repeated_redaction_is_idempotent_and_does_not_recount_placeholders() {
        let input = "api_key=not-real password='not real' \"token\":\"fake\" \
            https://user:pass@example.invalid/path Bearer fake \
            Ab3dEf6hIj9lMn2pQr5tUv8xYz1BC4DE /Users/test/code";
        let first = DeterministicRedactor.redact(input);
        let second = DeterministicRedactor.redact(&first.text);
        assert!(first.finding_count() >= 6);
        assert_eq!(first.text, second.text);
        assert_eq!(second.finding_count(), 0);
        assert_eq!(DeterministicRedactor.ruleset_version(), 2);
    }

    #[test]
    fn handles_unicode_and_large_candidates_without_fragmenting_the_secret() {
        let token = "Ab3dEf6hIj9lMn2pQr5tUv8xYz1BC4DE".repeat(32768);
        let result = DeterministicRedactor.redact(&format!("前缀 {token} 后缀"));
        assert_eq!(result.text, "前缀 [REDACTED_HIGH_ENTROPY] 后缀");
        assert_eq!(result.finding_count(), 1);
    }

    #[test]
    fn collision_thresholds_use_exact_integer_comparisons() {
        assert!(high_entropy_candidate("0123456789abcdef0123456789abcdef"));
        assert!(high_entropy_candidate("Ab3dEf6hIj9lMn2pQr5tUv8xYz1BC4DE"));
        assert!(!high_entropy_candidate("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa1"));
    }

    #[test]
    fn collapses_user_home_paths_but_keeps_the_project_relative_suffix() {
        let result = DeterministicRedactor
            .redact("/Users/alice/Workspace/project and C:\\Users\\bob\\code\\project");
        assert_eq!(result.text, "~/Workspace/project and ~\\code\\project");
        assert_eq!(result.finding_counts.get("home_path"), Some(&2));
    }
}
