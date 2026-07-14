#![forbid(unsafe_code)]
//! # cherry-pit-merger
//!
//! Canonical command-side EDA primitive: a single-task merger holding the sole
//! `EventStore` write handle for one aggregate substrate. Consumers implement
//! [`MergerArm`], supplying the persist-mode decision and a pure command
//! handler; the crate owns load, handle, create-or-append, publish and the
//! I1 TOCTOU resolution. Rationale, [`PersistMode`] shapes, and the
//! regression-pin contract are governed by [CHE-0069], not restated.
//!
//! Per [CHE-0029] the crate depends only on `cherry-pit-core` and `tokio`: a
//! sibling of `cherry-pit-app`, not a downstream.
//!
//! ```text
//! load → handle → create-or-append → publish
//! ```
//!
//! TOCTOU: every persist call lives inside the merger task's `run` loop,
//! awaiting each command's full triad before dequeuing the next, so same-key
//! callers serialise at the `mpsc` front door (CHE-0069:R4).
//!
//! Wiring: implement [`MergerArm`], build the persist handles, pass to
//! [`Merger::spawn`], dispatch via [`MergerHandle`].
//!
//! [CHE-0069]: https://github.com/acje/solon/blob/main/docs/adr/cherry/CHE-0069-cherry-pit-merger.md
//! [CHE-0029]: https://github.com/acje/solon/blob/main/docs/adr/cherry/CHE-0029-cargo-workspace-crate-dag.md

mod arm;
mod command;
mod handle;
mod merger;
mod shared;

pub use arm::{MergerArm, PersistMode};
pub use command::MergerCommand;
pub use handle::MergerHandle;
pub use merger::{MERGER_CHANNEL_CAPACITY, Merger};
