//! Ephemeral reader leases. Release belongs to a reader, not to the snapshot.
//! Retain ordering tombstones until snapshot eviction so late renewals cannot
//! resurrect a closed reader. Bounds follow the snapshot cache, not history.
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};
const TTL: Duration = Duration::from_mins(2);
const READERS_PER_SNAPSHOT: usize = 256;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LeaseRequest {
    pub task_id: String,
    pub snapshot_id: String,
    pub lease_id: String,
    pub revision: u64,
    pub release: bool,
}
struct Reader {
    revision: u64,
    released: bool,
    expires: Instant,
}
#[derive(Default)]
struct Snapshot {
    initial_grace: Option<Instant>,
    readers: BTreeMap<String, Reader>,
}
#[derive(Default)]
pub(super) struct SnapshotLeases(BTreeMap<String, Snapshot>);
impl SnapshotLeases {
    pub fn publish(&mut self, snapshot: &str, now: Instant) {
        self.0.insert(
            snapshot.into(),
            Snapshot {
                initial_grace: Some(now + TTL),
                ..Default::default()
            },
        );
    }
    pub fn remove(&mut self, snapshot: &str) {
        self.0.remove(snapshot);
    }
    pub fn retain(&mut self, mut keep: impl FnMut(&str) -> bool) {
        self.0.retain(|id, _| keep(id));
    }
    pub fn protected(&self, snapshot: &str, now: Instant) -> bool {
        self.0.get(snapshot).is_some_and(|state| {
            state.initial_grace.is_some_and(|expiry| expiry > now)
                || state
                    .readers
                    .values()
                    .any(|reader| !reader.released && reader.expires > now)
        })
    }
    pub fn update(&mut self, request: &LeaseRequest, now: Instant) -> Result<(), String> {
        uuid::Uuid::parse_str(&request.lease_id).map_err(|_| "无效的比较阅读租约")?;
        if request.revision == 0 {
            return Err("无效的比较租约版本".into());
        }
        let state = self
            .0
            .get_mut(&request.snapshot_id)
            .ok_or("比较结果已过期，请刷新")?;
        if let Some(reader) = state.readers.get(&request.lease_id) {
            if request.revision < reader.revision {
                return Ok(());
            }
            if request.revision == reader.revision {
                return if request.release == reader.released {
                    Ok(())
                } else {
                    Err("比较租约版本已用于不同操作".into())
                };
            }
            if reader.released && !request.release {
                return Err("此阅读租约已释放，请重新打开比较".into());
            }
        } else if state.readers.len() >= READERS_PER_SNAPSHOT {
            return Err("此比较的阅读租约过多，请刷新比较后重试".into());
        }
        // The publication grace only bridges the first reader's acquisition.
        // Existing readers now protect themselves independently.
        state.initial_grace = None;
        state.readers.insert(
            request.lease_id.clone(),
            Reader {
                revision: request.revision,
                released: request.release,
                expires: now + TTL,
            },
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request(id: &str, revision: u64, release: bool) -> LeaseRequest {
        LeaseRequest {
            task_id: "task".into(),
            snapshot_id: "snapshot".into(),
            lease_id: id.into(),
            revision,
            release,
        }
    }
    #[test]
    fn independent_readers_do_not_release_each_other_and_expire_without_renewal() {
        let mut leases = SnapshotLeases::default();
        let now = Instant::now();
        leases.publish("snapshot", now);
        let a = uuid::Uuid::new_v4().to_string();
        let b = uuid::Uuid::new_v4().to_string();
        leases.update(&request(&a, 1, false), now).unwrap();
        leases.update(&request(&b, 1, false), now).unwrap();
        leases.update(&request(&a, 2, true), now).unwrap();
        assert!(leases.protected("snapshot", now));
        leases
            .update(&request(&b, 2, false), now + Duration::from_secs(90))
            .unwrap();
        assert!(leases.protected("snapshot", now + Duration::from_secs(121)));
        assert!(!leases.protected("snapshot", now + Duration::from_secs(211)));
    }
    #[test]
    fn delayed_renewal_cannot_recreate_a_released_reader_even_if_it_arrives_first() {
        let now = Instant::now();
        let mut leases = SnapshotLeases::default();
        leases.publish("snapshot", now);
        let id = uuid::Uuid::new_v4().to_string();
        leases.update(&request(&id, 3, true), now).unwrap();
        leases.update(&request(&id, 2, false), now).unwrap();
        leases.update(&request(&id, 1, false), now).unwrap();
        assert!(!leases.protected("snapshot", now));
        assert!(leases.update(&request(&id, 3, false), now).is_err());
        assert!(leases.update(&request(&id, 4, false), now).is_err());
        leases.update(&request(&id, 3, true), now).unwrap();
        assert!(!leases.protected("snapshot", now));
    }
    #[test]
    fn repeated_ack_does_not_extend_expiry_and_invalid_or_unbounded_readers_are_rejected() {
        let now = Instant::now();
        let mut leases = SnapshotLeases::default();
        leases.publish("snapshot", now);
        assert!(leases.update(&request("bad-id", 1, false), now).is_err());
        let id = uuid::Uuid::new_v4().to_string();
        assert!(leases.update(&request(&id, 0, false), now).is_err());
        leases.update(&request(&id, 1, false), now).unwrap();
        leases
            .update(&request(&id, 1, false), now + Duration::from_secs(100))
            .unwrap();
        assert!(!leases.protected("snapshot", now + Duration::from_secs(121)));
        for _ in 1..READERS_PER_SNAPSHOT {
            leases
                .update(&request(&uuid::Uuid::new_v4().to_string(), 1, true), now)
                .unwrap();
        }
        assert!(
            leases
                .update(&request(&uuid::Uuid::new_v4().to_string(), 1, false), now)
                .is_err()
        );
        leases.remove("snapshot");
        assert!(leases.update(&request(&id, 2, false), now).is_err());
        assert!(leases.0.is_empty());
    }
}
