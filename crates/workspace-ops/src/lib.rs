//! Git, review and file operations for a project's directories. The same code answers
//! for this machine's projects in the service and for a server's projects in the shim
//! running there, so behaviour never depends on where a project lives. What it needs
//! to know about tasks comes through [`ReviewFacts`].
pub mod facts;
pub mod operation;
pub mod project_scan;
pub mod review;
pub mod review_file_actions;
pub mod state_changes;
pub mod turn_undo;
pub mod workspace_service;

#[cfg(feature = "core")]
pub mod core_facts;

pub use agent_workspace::{extend_git_search_path, git_is_installed};
pub use facts::{MutationGuard, ReviewFacts, VerifiedArtifact};
