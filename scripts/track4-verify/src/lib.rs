//! track4-verify — mechanical verifier for Phase 2 v2 Track 4.
//!
//! Library surface exists only to give unit tests something to import.
//! Real entry point is `main.rs`.

#![forbid(unsafe_code)]

pub mod criteria;
pub mod runner;

use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Pass,
    Fail,
    Manual,
}

impl Verdict {
    #[must_use]
    pub fn tag(self) -> &'static str {
        match self {
            Self::Pass => "PASS",
            Self::Fail => "FAIL",
            Self::Manual => "MANUAL",
        }
    }
}

#[derive(Debug)]
pub struct CriterionResult {
    pub verdict: Verdict,
    pub metric: String,
    pub note: String,
    pub duration_ms: u128,
}

pub struct Context {
    pub workspace_root: PathBuf,
    pub eventstore_ceiling: usize,
    pub strict_docs: bool,
}

pub struct Criterion {
    pub num: &'static str,
    pub short_name: &'static str,
    pub runner: fn(&Context) -> CriterionResult,
}
