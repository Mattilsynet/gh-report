//! Append-only event log with fiber lookup.
//!
//! The dragline crate-internal tree is split across sibling files along
//! concern boundaries:
//!
//! - `all` (transient, removed at the end of M1): holds the not-yet-split
//!   remainder during the staged refactor.
//! - Future siblings: `linevec`, `commit`, `state`, `api` — see
//!   `AFM`-/`PAR`-governed split (mission
//!   `pardosa-wave1-m1-split-dragline-1779100805`).
//!
//! Public re-exports below preserve the exact pre-split surface
//! (`pardosa::Dragline`, `pardosa::AppendResult`).

mod all;
mod commit;
mod linevec;

pub use all::{AppendResult, Dragline};
