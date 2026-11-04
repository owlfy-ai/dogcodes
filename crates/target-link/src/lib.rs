//! Reaches a saved machine through the system `ssh`, deploys `dogcodes-shim` over
//! that same connection and keeps a framed control channel open to it.
pub mod askpass;
mod boot;
mod connect;
mod error;
mod link;
mod program;
mod shims;
pub mod ssh_config;

pub use askpass::{AskpassPrompt, PromptKind};
pub use connect::{ConnectObserver, ConnectRequest, ConnectStep, connect};
pub use error::{ConnectError, ConnectErrorKind};
pub use link::TargetLink;
pub use program::{SshProgram, find_ssh};
pub use shims::ShimCatalog;
pub use target_wire::TargetInfo;
