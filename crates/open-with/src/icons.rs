use super::Candidate;
use base64::{Engine, engine::general_purpose::STANDARD};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant, SystemTime},
};

type Key = (String, PathBuf, Option<SystemTime>);
struct Entry {
    created: Instant,
    data: Option<String>,
}
static CACHE: OnceLock<Mutex<HashMap<Key, Entry>>> = OnceLock::new();

// Share decoded application identities across PDF/HTML cards. App name alone
// is not a stable key: separate installed apps can have the same display name.
pub(super) fn cached_icon(
    candidate: &Candidate,
    load: impl FnOnce() -> Option<Vec<u8>>,
) -> Option<String> {
    let key = (
        candidate.app.id.clone(),
        candidate.location.clone(),
        std::fs::metadata(&candidate.location)
            .and_then(|m| m.modified())
            .ok(),
    );
    let mut cache = CACHE.get_or_init(Default::default).lock().ok()?;
    cache.retain(|_, value| {
        value.created.elapsed() < Duration::from_secs(if value.data.is_some() { 300 } else { 30 })
    });
    if let Some(value) = cache.get(&key) {
        return value.data.clone();
    }
    let data = load()
        .filter(|bytes| bytes.starts_with(b"\x89PNG\r\n\x1a\n") && bytes.len() <= 256 * 1024)
        .map(|bytes| format!("data:image/png;base64,{}", STANDARD.encode(bytes)));
    if cache.len() >= 128
        && let Some(oldest) = cache
            .iter()
            .min_by_key(|(_, entry)| entry.created)
            .map(|(key, _)| key.clone())
    {
        cache.remove(&oldest);
    }
    cache.insert(
        key,
        Entry {
            created: Instant::now(),
            data: data.clone(),
        },
    );
    data
}

#[cfg(test)]
mod tests {
    use super::*;
    fn candidate(id: &str, path: &str) -> Candidate {
        Candidate {
            app: crate::Application {
                id: id.into(),
                name: "Same name".into(),
                is_default: false,
                icon: None,
            },
            location: path.into(),
            visible: true,
            recommended: true,
        }
    }
    #[test]
    fn caches_per_identity_and_path_not_display_name() {
        let app = candidate("cache-a", "/one");
        assert!(cached_icon(&app, || Some(b"\x89PNG\r\n\x1a\nfixture".to_vec())).is_some());
        assert!(cached_icon(&app, || panic!("cached icon must not reload")).is_some());
        assert!(cached_icon(&candidate("cache-a", "/two"), || None).is_none());
        assert!(cached_icon(&candidate("cache-b", "/one"), || None).is_none());
    }
    #[test]
    fn rejects_non_png_and_remembers_missing_icons() {
        let app = candidate("bad-icon", "/invalid");
        assert!(cached_icon(&app, || Some(b"not an image".to_vec())).is_none());
        assert!(cached_icon(&app, || panic!("negative cache must prevent retries")).is_none());
    }
}
