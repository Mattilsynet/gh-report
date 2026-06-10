//! [`MergerCommand`] — channel envelope carrying one command from a
//! caller into the merger task.
//!
//! Each command bundles the caller's pure command payload, the
//! per-dispatch [`CorrelationContext`] (CHE-0039:R1), and a
//! [`oneshot::Sender`] reply channel typed to the arm's error so
//! call-site `.await? -> Result<(), Arm::Err>` semantics are
//! preserved verbatim.
//!
//! The struct is parameterised on `<A, Arm>` (not just `<Arm>`) to
//! keep the `A: Aggregate` bound visible at the channel boundary —
//! consumers can name the channel item type as
//! `MergerCommand<MyAggregate, MyArm>` without manual reach into
//! associated types.

use cherry_pit_core::{Aggregate, CorrelationContext};
use tokio::sync::oneshot;

use crate::arm::MergerArm;

/// Channel envelope: one command + reply, in transit from a caller
/// into the merger task.
///
/// The merger consumes a [`mpsc::Receiver<MergerCommand<A, Arm>>`]
/// and routes each item through the arm's
/// [`MergerArm::persist_mode`] + [`MergerArm::handle`] step before
/// firing the reply.
///
/// [`mpsc::Receiver<MergerCommand<A, Arm>>`]: https://docs.rs/tokio/latest/tokio/sync/mpsc/struct.Receiver.html
#[derive(Debug)]
pub struct MergerCommand<A, Arm>
where
    A: Aggregate,
    Arm: MergerArm<A>,
{
    /// Caller's pure command payload.
    pub cmd: Arm::Cmd,
    /// Per-dispatch correlation context. Threaded into
    /// [`EventStore::create`] / [`EventStore::append`] so each
    /// persisted envelope carries the `(correlation_id,
    /// causation_id)` pair from the command's originating boundary
    /// per CHE-0039:R1.
    ///
    /// [`EventStore::create`]: cherry_pit_core::EventStore::create
    /// [`EventStore::append`]: cherry_pit_core::EventStore::append
    pub ctx: CorrelationContext,
    /// Caller-supplied reply channel. The merger sends
    /// `Ok(())` on a successful triad or `Err(Arm::Err)` on any
    /// failure (load, handle, persist, missing-key); a failure to
    /// send (caller dropped the receiver) is logged and absorbed —
    /// the persistence work already completed and the error is
    /// informational at that point.
    pub reply: oneshot::Sender<Result<(), Arm::Err>>,
}
