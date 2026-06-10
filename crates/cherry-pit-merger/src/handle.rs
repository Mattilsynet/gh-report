//! [`MergerHandle`] — the public dispatch surface returned by
//! [`crate::Merger::spawn`].
//!
//! Wraps a [`mpsc::Sender<MergerCommand<A, Arm>>`] and exposes
//! [`MergerHandle::dispatch`], the single async call that crosses the
//! channel boundary, awaits the merger's [`oneshot`] reply, and
//! returns the arm's [`Result<(), Arm::Err>`] verbatim.
//!
//! The handle is [`Clone`] (cheap [`Arc`]-equivalent under
//! [`mpsc::Sender`]'s own clone semantics) so consumers can spread
//! it across services that all dispatch into the same merger
//! substrate per [CHE-0005:R1] single-aggregate-per-port.
//!
//! [`mpsc::Sender<MergerCommand<A, Arm>>`]: https://docs.rs/tokio/latest/tokio/sync/mpsc/struct.Sender.html
//! [`oneshot`]: https://docs.rs/tokio/latest/tokio/sync/oneshot/index.html
//! [CHE-0005:R1]: https://github.com/acje/solon/blob/main/docs/adr/cherry/CHE-0005-single-aggregate-design.md

use cherry_pit_core::{Aggregate, CorrelationContext, StoreError};
use tokio::sync::{mpsc, oneshot};

use crate::arm::MergerArm;
use crate::command::MergerCommand;

/// Public dispatch surface for a spawned [`crate::Merger`].
///
/// Cloning is cheap (clones the underlying [`mpsc::Sender`]); spread
/// the handle across services that all need to dispatch into the
/// same merger.
pub struct MergerHandle<A, Arm>
where
    A: Aggregate,
    Arm: MergerArm<A>,
{
    tx: mpsc::Sender<MergerCommand<A, Arm>>,
}

impl<A, Arm> Clone for MergerHandle<A, Arm>
where
    A: Aggregate,
    Arm: MergerArm<A>,
{
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
        }
    }
}

impl<A, Arm> MergerHandle<A, Arm>
where
    A: Aggregate,
    Arm: MergerArm<A>,
{
    pub(crate) fn new(tx: mpsc::Sender<MergerCommand<A, Arm>>) -> Self {
        Self { tx }
    }

    /// Dispatch `cmd` to the merger and await the triad result.
    ///
    /// The future resolves to:
    ///
    /// - `Ok(())` once the merger has loaded + folded + handled +
    ///   persisted + published (or absorbed the publish failure per
    ///   CHE-0024:R1).
    /// - `Err(Arm::Err)` on any failure surfaced by the arm
    ///   ([`MergerArm::handle`] domain error), the store
    ///   ([`StoreError`] lifted through [`MergerArm::Err`]'s
    ///   `From<StoreError>` bound), or the routing-index
    ///   ([`MergerArm::missing_key_error`] for
    ///   [`crate::PersistMode::AppendStrict`] misses).
    ///
    /// # Errors
    ///
    /// Returns [`MergerArm::Err`] for the failure cases above. The
    /// merger-internal "channel send failed" and "reply channel
    /// dropped" cases are lifted into
    /// [`StoreError::Infrastructure`] (retryable per CHE-0046:R1)
    /// and surfaced through the same path — callers see one error
    /// type for both domain and infrastructure failures.
    pub async fn dispatch(&self, cmd: Arm::Cmd, ctx: CorrelationContext) -> Result<(), Arm::Err> {
        let (reply_tx, reply_rx) = oneshot::channel();
        let envelope = MergerCommand {
            cmd,
            ctx,
            reply: reply_tx,
        };
        self.tx.send(envelope).await.map_err(|_send_err| {
            Arm::Err::from(StoreError::Infrastructure(
                "MergerHandle::dispatch: merger task channel closed; \
                 the merger task has shut down and is no longer accepting commands"
                    .into(),
            ))
        })?;
        match reply_rx.await {
            Ok(result) => result,
            Err(_recv_err) => Err(Arm::Err::from(StoreError::Infrastructure(
                "MergerHandle::dispatch: merger task dropped the reply channel \
                 before sending a result; the triad outcome is unobservable"
                    .into(),
            ))),
        }
    }
}
