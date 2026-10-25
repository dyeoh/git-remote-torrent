//! Thin wrapper over git plumbing commands.
//!
//! Commands run through the user's installed `git` binary so behaviour
//! matches their git version and configuration exactly.

mod error;
mod pack;
mod repo;

pub use crate::error::Error;
pub use crate::repo::{Repo, global_config, set_global_config};
