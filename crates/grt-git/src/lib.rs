//! Thin wrapper over git plumbing (`rev-list`, `pack-objects`, `index-pack`,
//! `for-each-ref`).
//!
//! Commands are run through the user's installed `git` binary so behaviour
//! matches their git version and configuration exactly.
