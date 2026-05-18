//! Append-only event log with fiber lookup.
//!
//! The dragline crate-internal tree is split across sibling files along
//! concern boundaries:
//!
//! - `linevec` — append-only `Linevec<T>` newtype + write-time validators.
//! - `commit` — `PreparedCommit<T>`, `LookupOp`, two-phase
//!   prepare/apply commit machinery (FH5).
//! - `state` — public `Dragline<T>` struct, `AppendResult`, constructors.
//! - `api` — the `impl Dragline { … }` public write/read surface.
//!
//! Public re-exports below preserve the exact pre-split surface
//! (`pardosa::Dragline`, `pardosa::AppendResult`).

mod api;
mod commit;
mod linevec;
mod state;

pub use state::{AppendResult, Dragline};
