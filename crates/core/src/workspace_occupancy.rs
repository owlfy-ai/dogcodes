//! Which conversations are running a turn in each folder.
//!
//! File changes are captured by comparing a folder before and after an
//! interval. When another conversation runs in the same folder during that
//! interval (every "no project" conversation shares one scratch folder), the
//! comparison also sees that conversation's writes, so the captured files
//! cannot be attributed to this turn and must not be offered for undo.
use agent_domain::{TargetId, TaskId};
use serde_json::{Value, json};
use std::{
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

/// Attribution of a capture whose folder another conversation used meanwhile.
pub(crate) const SHARED_ATTRIBUTION: &str = "shared_workspace_interval";

#[derive(Default)]
pub(crate) struct WorkspaceOccupancy {
    entries: Mutex<Vec<Arc<Entry>>>,
}

struct Entry {
    task: TaskId,
    target: Option<TargetId>,
    paths: Vec<String>,
    /// Bumped each time another conversation starts in an overlapping folder.
    arrivals: AtomicU64,
}

impl Entry {
    fn overlaps(&self, other: &Entry) -> bool {
        self.task != other.task
            && self.target == other.target
            && folders_overlap(&self.paths, &other.paths)
    }
}

/// One running turn's claim on its folders; released when dropped.
pub(crate) struct Presence {
    registry: Arc<WorkspaceOccupancy>,
    entry: Arc<Entry>,
}

/// The start of a capture interval.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Mark {
    others_present: bool,
    arrivals: u64,
}

impl WorkspaceOccupancy {
    pub(crate) fn enter(
        self: &Arc<Self>,
        task: TaskId,
        target: Option<TargetId>,
        paths: Vec<String>,
    ) -> Presence {
        let entry = Arc::new(Entry {
            task,
            target,
            paths,
            arrivals: AtomicU64::new(0),
        });
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for other in entries.iter().filter(|other| other.overlaps(&entry)) {
            other.arrivals.fetch_add(1, Ordering::AcqRel);
        }
        entries.push(entry.clone());
        Presence {
            registry: self.clone(),
            entry,
        }
    }
}

impl Presence {
    pub(crate) fn mark(&self) -> Mark {
        let entries = self
            .registry
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Mark {
            others_present: entries.iter().any(|other| other.overlaps(&self.entry)),
            arrivals: self.entry.arrivals.load(Ordering::Acquire),
        }
    }

    /// Whether another conversation used an overlapping folder at any time
    /// since `mark`.
    pub(crate) fn shared_since(&self, mark: &Mark) -> bool {
        mark.others_present || self.entry.arrivals.load(Ordering::Acquire) != mark.arrivals
    }
}

impl Drop for Presence {
    fn drop(&mut self) {
        self.registry
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|entry| !Arc::ptr_eq(entry, &self.entry));
    }
}

/// Whether two sets of project folders on the same machine overlap.
pub(crate) fn folders_overlap(a: &[String], b: &[String]) -> bool {
    a.iter().any(|a| {
        b.iter().any(|b| {
            let (a, b) = (Path::new(a), Path::new(b));
            within(a, b) || within(b, a)
        })
    })
}

/// Whether captures of `outer` see `inner`. They skip a folder marked as
/// holding conversations' own folders (see `conversation_drafts`).
fn within(inner: &Path, outer: &Path) -> bool {
    inner.starts_with(outer)
        && !inner
            .ancestors()
            .take_while(|folder| *folder != outer)
            .any(|folder| {
                folder
                    .join(agent_protocol::workspace_context::CONVERSATION_FOLDERS_MARKER)
                    .exists()
            })
}

/// Records that the capture may include another conversation's writes.
pub(crate) fn mark_shared(change: &mut Value) {
    change["attribution"] = json!(SHARED_ATTRIBUTION);
    change["sharedWorkspace"] = json!(true);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: &str) -> TaskId {
        TaskId::parse(id).unwrap()
    }
    const A: &str = "tsk_01M71KJBVDD6DB3A9E1K0FCR8R";
    const B: &str = "tsk_01M6ZW7JK32Z3QY6PHCHJ7VCBZ";

    #[test]
    fn a_turn_alone_in_its_folder_is_not_shared() {
        let registry = Arc::new(WorkspaceOccupancy::default());
        let one = registry.enter(task(A), None, vec!["/scratch".into()]);
        let mark = one.mark();
        // A different folder, another machine, or the same conversation do not count.
        drop(registry.enter(task(B), None, vec!["/project".into()]));
        drop(registry.enter(task(B), Some(TargetId::new()), vec!["/scratch".into()]));
        drop(registry.enter(task(A), None, vec!["/scratch".into()]));
        assert!(!one.shared_since(&mark));
    }

    #[test]
    fn a_conversation_starting_or_already_running_in_the_folder_shares_it() {
        let registry = Arc::new(WorkspaceOccupancy::default());
        let one = registry.enter(task(A), None, vec!["/scratch".into()]);
        let mark = one.mark();
        // It may already have finished when this turn's capture ends.
        drop(registry.enter(task(B), None, vec!["/scratch/sub".into()]));
        assert!(one.shared_since(&mark));
        assert!(!one.shared_since(&one.mark()));

        let two = registry.enter(task(B), None, vec!["/scratch".into()]);
        let mark = two.mark();
        assert!(two.shared_since(&mark));
        drop(one);
        assert!(!two.shared_since(&two.mark()));
    }

    #[test]
    fn folders_overlap_by_whole_path_components() {
        assert!(folders_overlap(&["/a".into()], &["/a/b".into()]));
        assert!(!folders_overlap(&["/a".into()], &["/ab".into()]));
    }

    #[test]
    fn conversation_folders_are_apart_from_the_scratch_folder_and_each_other() {
        let scratch = tempfile::tempdir().unwrap();
        let root = scratch.path().to_string_lossy().into_owned();
        let drafts = scratch.path().join("drafts");
        let (one, two) = (drafts.join("one"), drafts.join("two"));
        let path = |p: &std::path::Path| vec![p.to_string_lossy().into_owned()];
        assert!(folders_overlap(std::slice::from_ref(&root), &path(&one)));
        std::fs::create_dir_all(&drafts).unwrap();
        std::fs::write(
            drafts.join(agent_protocol::workspace_context::CONVERSATION_FOLDERS_MARKER),
            "",
        )
        .unwrap();
        assert!(!folders_overlap(&[root], &path(&one)));
        assert!(!folders_overlap(&path(&one), &path(&two)));
        assert!(folders_overlap(&path(&one), &path(&one.join("sub"))));
    }
}
