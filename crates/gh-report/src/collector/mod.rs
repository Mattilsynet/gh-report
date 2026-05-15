//! Repository security check collectors.
//!
//! Each submodule implements the evaluation logic for one security check.
//! Organization-level metric aggregation lives in [`crate::aggregate::metrics`].

pub mod branch_protection;
pub mod codeowners;
pub mod codeowners_parser;
pub mod dependabot;
pub mod ghas_scanning;
pub mod inventory;
pub mod last_commit;
pub mod ref_matching;
pub mod security_policy;
