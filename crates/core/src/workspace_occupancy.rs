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

/// Scoped undo and running turns exclude one another only where their
/// workspaces may overlap. The global lock still coordinates other Review writes.
#[derive(Default)]
pub(super) struct UndoGate {
    leases: Mutex<Vec<Arc<Lease>>>,
    released: tokio::sync::Notify,
}

pub(super) enum Workspace {
    Unknown,
    Local(Option<Vec<std::path::PathBuf>>),
    Remote(TargetId),
}

impl Workspace {
    pub(super) fn resolve(target: Option<&TargetId>, paths: &[String]) -> Self {
        if let Some(target) = target {
            // The hub cannot resolve filesystem aliases on a remote machine.
            return Self::Remote(target.clone());
        }
        Self::Local(
            paths
                .iter()
                .map(|path| {
                    let path = std::fs::canonicalize(path).ok()?;
                    if cfg!(any(target_os = "macos", windows)) {
                        Some(path.to_string_lossy().to_lowercase().into())
                    } else {
                        Some(path)
                    }
                })
                .collect(),
        )
    }

    fn overlaps(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Unknown, _) | (_, Self::Unknown) => true,
            (Self::Local(Some(a)), Self::Local(Some(b))) => a
                .iter()
                .any(|a| b.iter().any(|b| a.starts_with(b) || b.starts_with(a))),
            (Self::Local(_), Self::Local(_)) => true,
            (Self::Remote(a), Self::Remote(b)) => a == b,
            _ => false,
        }
    }
}

struct Lease {
    workspace: Workspace,
    undo: bool,
}

pub(super) struct UndoLease {
    gate: Arc<UndoGate>,
    lease: Arc<Lease>,
}

impl UndoGate {
    fn try_enter(self: &Arc<Self>, lease: &Arc<Lease>) -> Option<UndoLease> {
        let mut leases = self.leases.lock().expect("workspace undo gate");
        if leases
            .iter()
            .any(|other| (lease.undo || other.undo) && lease.workspace.overlaps(&other.workspace))
        {
            return None;
        }
        leases.push(lease.clone());
        Some(UndoLease {
            gate: self.clone(),
            lease: lease.clone(),
        })
    }

    pub(super) fn try_undo(self: &Arc<Self>, workspace: Workspace) -> Option<UndoLease> {
        self.try_enter(&Arc::new(Lease {
            workspace,
            undo: true,
        }))
    }

    pub(super) async fn enter_turn(self: &Arc<Self>, workspace: Workspace) -> UndoLease {
        let lease = Arc::new(Lease {
            workspace,
            undo: false,
        });
        loop {
            let released = self.released.notified();
            if let Some(guard) = self.try_enter(&lease) {
                return guard;
            }
            released.await;
        }
    }
}

impl Drop for UndoLease {
    fn drop(&mut self) {
        self.gate
            .leases
            .lock()
            .expect("workspace undo gate")
            .retain(|lease| !Arc::ptr_eq(lease, &self.lease));
        if self.lease.undo {
            self.gate.released.notify_waiters();
        }
    }
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

    #[tokio::test]
    async fn undo_only_excludes_turns_and_undos_in_overlapping_workspaces() {
        let gate = Arc::new(UndoGate::default());
        let workspace = tempfile::tempdir().unwrap();
        let a = workspace.path().join("a");
        let nested = a.join("nested");
        let b = workspace.path().join("b");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::create_dir(&b).unwrap();
        let scope = |path: &Path| Workspace::resolve(None, &[path.to_str().unwrap().into()]);
        let run = gate.enter_turn(scope(&nested)).await;
        assert!(gate.try_undo(scope(&a)).is_none());
        let undo_b = gate
            .try_undo(scope(&b))
            .expect("unrelated run must not block undo");
        assert!(gate.try_undo(scope(&b)).is_none());
        let turn_b = gate.enter_turn(scope(&b));
        tokio::pin!(turn_b);
        assert!(futures::poll!(&mut turn_b).is_pending());
        drop(undo_b);
        let _turn_b = turn_b.await;
        drop(run);
        assert!(gate.try_undo(scope(&a)).is_some());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn undo_detects_a_running_workspace_opened_through_a_symlink() {
        let gate = Arc::new(UndoGate::default());
        let root = tempfile::tempdir().unwrap();
        let alias = root.path().join("alias");
        let actual = root.path().join("actual");
        std::fs::create_dir(&actual).unwrap();
        std::os::unix::fs::symlink(&actual, &alias).unwrap();
        let scope = |path: &Path| Workspace::resolve(None, &[path.to_str().unwrap().into()]);
        let _turn = gate.enter_turn(scope(&alias)).await;
        assert!(gate.try_undo(scope(&actual)).is_none());
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
