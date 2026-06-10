#![forbid(unsafe_code)]
//! # cherry-pit-merger
//!
//! Canonical command-side EDA primitive: a single-task command merger
//! that holds the sole [`EventStore`] write handle for one aggregate
//! substrate. Consumers supply a per-aggregate [`MergerArm`] impl that
//! carries the routing decision and the pure command handler; the
//! merger crate owns the load → handle → create-or-append → publish
//! triad and the persist-side I1 TOCTOU resolution.
//!
//! ## Why a separate crate
//!
//! The merger is the cherry-pit substrate's command-side serialiser:
//! every consumer (gh-report today; future cherry-pit-app consumers
//! tomorrow) wants the same load → handle → persist → publish loop,
//! with the same TOCTOU-safe lazy-create-or-append semantics, against
//! its own aggregate types. Lifting the merger out of any single
//! consumer crate gives one place to encode the pattern and one place
//! to test the persist-side invariant.
//!
//! Per [CHE-0029] this crate depends only on [`cherry-pit-core`] and
//! [`tokio`] — it is a sibling of [`cherry-pit-app`], **not** a
//! downstream of it, so consumers may wire it into any composition
//! without picking up the full agent surface.
//!
//! ## What the merger is
//!
//! A single [`tokio::task`] consuming a bounded [`mpsc::Receiver`] of
//! [`MergerCommand`] envelopes. Each command carries the caller's
//! pure command payload plus a [`oneshot::Sender`] reply so call-site
//! `.await? -> Result<(), Arm::Err>` semantics are preserved. The
//! task body runs the canonical EDA triad:
//!
//! ```text
//! load → handle → create-or-append → publish
//! ```
//!
//! per command, in dispatch order, surfacing every error through the
//! reply channel. The arm's [`MergerArm::persist_mode`] picks the
//! routing strategy (fresh aggregate per call, lazy create-or-append
//! by domain key, or strict append against an existing aggregate)
//! per command.
//!
//! ## I1 TOCTOU resolution (canonical doctrine)
//!
//! In isolation, "look up `domain_key` in the routing index, then
//! call `EventStore::create` on a miss, then `index.or_insert` the
//! assigned id" is a check-then-act sequence: two concurrent
//! same-`domain_key` callers could both observe the lookup miss,
//! both create a fresh aggregate, and only one of the
//! `or_insert` calls would win — leaving an **orphan stream** on
//! disk that the index never points back to.
//!
//! The merger closes this window structurally: every persist call
//! site lives inside the merger task's `run` loop, which awaits each
//! command's full triad before dequeuing the next. Two concurrent
//! same-key callers serialise at the [`mpsc`] front door; the second
//! observer always sees the first creator's index entry, so exactly
//! one [`EventStore::create`] fires per key. The brief
//! [`std::sync::Mutex`] guard on the routing index is taken only to
//! perform the `or_insert` itself and is released before any `await`
//! on storage I/O.
//!
//! This is the "per-domain-key single-flight" requirement at the
//! coarsest granularity the sole-writer assumption permits. Sharding
//! the merger (per-key locks, partitioned index) becomes interesting
//! only when contention on the single mpsc front-door is measured;
//! out of scope here.
//!
//! ## Wiring at a glance
//!
//! 1. Implement [`MergerArm`] for your aggregate's command enum —
//!    the trait's three required methods are [`MergerArm::persist_mode`]
//!    (returns one of [`PersistMode::Create`],
//!    [`PersistMode::CreateOrAppend`], [`PersistMode::AppendStrict`]),
//!    [`MergerArm::handle`] (the pure load-then-handle step), and
//!    [`MergerArm::publish_label`] (the static label used in
//!    bus-failure log records).
//! 2. Build the merger's persist handles (the [`EventStore`], the
//!    [`EventBus`], the routing index, the per-aggregate sequence
//!    tracker) and pass them to [`Merger::spawn`].
//! 3. Use the returned [`MergerHandle`] to dispatch commands;
//!    `.await` on the returned future yields `Result<(), Arm::Err>`.
//!
//! [CHE-0029]: https://github.com/acje/solon/blob/main/docs/adr/cherry/CHE-0029-cargo-workspace-crate-dag.md
//! [`cherry-pit-core`]: cherry_pit_core
//! [`cherry-pit-app`]: https://github.com/acje/solon/tree/main/crates/cherry-pit-app
//! [`EventStore`]: cherry_pit_core::EventStore
//! [`EventBus`]: cherry_pit_core::EventBus
//! [`tokio::task`]: https://docs.rs/tokio/latest/tokio/task/index.html
//! [`mpsc`]: https://docs.rs/tokio/latest/tokio/sync/mpsc/index.html
//! [`mpsc::Receiver`]: https://docs.rs/tokio/latest/tokio/sync/mpsc/struct.Receiver.html
//! [`oneshot::Sender`]: https://docs.rs/tokio/latest/tokio/sync/oneshot/struct.Sender.html
//! [`EventStore::create`]: cherry_pit_core::EventStore::create

mod arm;
mod command;
mod handle;
mod merger;
mod shared;

pub use arm::{MergerArm, PersistMode};
pub use command::MergerCommand;
pub use handle::MergerHandle;
pub use merger::{MERGER_CHANNEL_CAPACITY, Merger};
