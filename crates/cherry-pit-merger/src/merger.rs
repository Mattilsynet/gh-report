//! [`Merger`] — single-task command merger that owns the sole
//! [`EventStore`] write handle for one aggregate substrate.
//!
//! Lifted from gh-report `app::services::merger` (cite: original at
//! `crates/gh-report/src/app/services/merger.rs:1..707`) and made
//! aggregate-agnostic via the [`MergerArm`] trait. Closes the I1
//! TOCTOU window structurally — see [`crate::shared`] module docs.
//!
//! [`EventStore`]: cherry_pit_core::EventStore

use std::collections::HashMap;
use std::num::NonZeroU64;
use std::sync::{Arc, Mutex};

use cherry_pit_core::{Aggregate, AggregateId, EventBus, EventStore};
use tokio::sync::mpsc;

use crate::arm::{MergerArm, PersistMode};
use crate::command::MergerCommand;
use crate::handle::MergerHandle;
use crate::shared::{
    self, PersistHandles, create_fresh, create_or_append, load_envelopes_or_empty, lookup,
};

/// Channel capacity for the merger command queue.
///
/// Sized large enough that bursty webhook ingestion + scheduled
/// progress checkpoints do not back-pressure call sites under normal
/// load. A full queue blocks the sender on
/// [`mpsc::Sender::send`]; the caller still observes its `.await`
/// semantics, so a brief stall is preferable to a `try_send` failure
/// path. Revisit when telemetry shows sustained queue depth.
///
/// Exposed at crate root so consumers can size dispatch sites or
/// dashboards against the same constant.
pub const MERGER_CHANNEL_CAPACITY: usize = 1024;

/// Single-task command merger. Constructed and immediately consumed
/// by [`Merger::spawn`]; the public handle is [`MergerHandle`].
///
/// The struct owns [`Arc`] clones of the persist-side handles plus
/// the consumer's [`MergerArm`] impl. The task body matches on
/// each incoming [`MergerCommand`], dispatches to the arm via
/// [`MergerArm::persist_mode`] + [`MergerArm::handle`], then drives
/// the load → handle → create-or-append → publish-or-trace triad
/// against the shared persist-side helpers in [`crate::shared`].
pub struct Merger<A, S, B, Arm>
where
    A: Aggregate,
    S: EventStore<Event = A::Event>,
    B: EventBus<Event = A::Event>,
    Arm: MergerArm<A>,
{
    arm: Arm,
    store: Arc<S>,
    bus: Arc<B>,
    index: Arc<Mutex<HashMap<String, AggregateId>>>,
    next_seq: Arc<Mutex<HashMap<AggregateId, NonZeroU64>>>,
    _aggregate: std::marker::PhantomData<A>,
}

impl<A, S, B, Arm> Merger<A, S, B, Arm>
where
    A: Aggregate,
    S: EventStore<Event = A::Event>,
    B: EventBus<Event = A::Event>,
    Arm: MergerArm<A>,
{
    /// Spawn the merger task and return a [`MergerHandle`] plus the
    /// underlying [`tokio::task::JoinHandle`]. Composition root holds
    /// the join handle to keep the task alive; the handle is the
    /// dispatch surface for every caller.
    ///
    /// The channel is bounded ([`MERGER_CHANNEL_CAPACITY`]); a
    /// saturated queue back-pressures producers via
    /// [`mpsc::Sender::send`] rather than dropping commands.
    #[must_use]
    pub fn spawn(
        arm: Arm,
        store: Arc<S>,
        bus: Arc<B>,
        index: Arc<Mutex<HashMap<String, AggregateId>>>,
        next_seq: Arc<Mutex<HashMap<AggregateId, NonZeroU64>>>,
    ) -> (MergerHandle<A, Arm>, tokio::task::JoinHandle<()>) {
        let merger = Self {
            arm,
            store,
            bus,
            index,
            next_seq,
            _aggregate: std::marker::PhantomData,
        };
        let (tx, rx) = mpsc::channel(MERGER_CHANNEL_CAPACITY);
        let join = tokio::spawn(merger.run(rx));
        (MergerHandle::new(tx), join)
    }

    /// Main task loop: receive commands and drive the triad.
    ///
    /// Channel close (every [`mpsc::Sender`] dropped — i.e. every
    /// [`MergerHandle`] has been dropped) exits the loop cleanly.
    /// Reply-side [`oneshot::Sender::send`] failure is swallowed
    /// (the caller dropped the receiver) — the persistence + publish
    /// work already completed and the error is informational at that
    /// point.
    ///
    /// [`oneshot::Sender::send`]: https://docs.rs/tokio/latest/tokio/sync/oneshot/struct.Sender.html#method.send
    async fn run(self, mut rx: mpsc::Receiver<MergerCommand<A, Arm>>) {
        while let Some(MergerCommand { cmd, ctx, reply }) = rx.recv().await {
            let result = self.handle_one(cmd, &ctx).await;
            let _ = reply.send(result);
        }
    }

    async fn handle_one(
        &self,
        cmd: Arm::Cmd,
        ctx: &cherry_pit_core::CorrelationContext,
    ) -> Result<(), Arm::Err> {
        let mode = self.arm.persist_mode(&cmd);
        let label = self.arm.publish_label(&cmd);
        match mode {
            PersistMode::Create => self.run_create(cmd, ctx, label).await,
            PersistMode::CreateOrAppend(key) => {
                self.run_create_or_append(key, cmd, ctx, label).await
            }
            PersistMode::AppendStrict(key) => self.run_append_strict(key, cmd, ctx, label).await,
        }
    }

    async fn run_create(
        &self,
        cmd: Arm::Cmd,
        ctx: &cherry_pit_core::CorrelationContext,
        label: &'static str,
    ) -> Result<(), Arm::Err> {
        let state = A::default();
        let new_events = self.arm.handle(&state, cmd)?;
        let envs = create_fresh(self.persist_handles(), None, new_events, ctx).await?;
        shared::publish_or_trace(&self.bus, &envs, label).await;
        Ok(())
    }

    async fn run_create_or_append(
        &self,
        key: String,
        cmd: Arm::Cmd,
        ctx: &cherry_pit_core::CorrelationContext,
        label: &'static str,
    ) -> Result<(), Arm::Err> {
        let existing_id = lookup(&self.index, &key);
        let (envelopes, last_seq) = load_envelopes_or_empty(&self.store, existing_id).await?;
        let mut state = A::default();
        for env in &envelopes {
            state.apply(env.payload());
        }
        let new_events = self.arm.handle(&state, cmd)?;
        let envs = create_or_append(
            self.persist_handles(),
            &key,
            existing_id,
            last_seq,
            new_events,
            ctx,
        )
        .await?;
        shared::publish_or_trace(&self.bus, &envs, label).await;
        Ok(())
    }

    async fn run_append_strict(
        &self,
        key: String,
        cmd: Arm::Cmd,
        ctx: &cherry_pit_core::CorrelationContext,
        label: &'static str,
    ) -> Result<(), Arm::Err> {
        let Some(id) = lookup(&self.index, &key) else {
            return Err(self.arm.missing_key_error(&key));
        };
        let envelopes = self.store.load(id).await?;
        let mut state = A::default();
        for env in &envelopes {
            state.apply(env.payload());
        }
        let last_seq = envelopes
            .last()
            .map(cherry_pit_core::EventEnvelope::sequence)
            .ok_or_else(|| {
                Arm::Err::from(cherry_pit_core::StoreError::CorruptData(
                    format!("indexed AggregateId {id} has zero envelopes (routing index stale)")
                        .into(),
                ))
            })?;
        let new_events = self.arm.handle(&state, cmd)?;
        let envs =
            shared::append_and_track(&self.store, &self.next_seq, id, last_seq, new_events, ctx)
                .await?;
        shared::publish_or_trace(&self.bus, &envs, label).await;
        Ok(())
    }

    fn persist_handles(&self) -> PersistHandles<'_, S, A::Event> {
        PersistHandles {
            store: &self.store,
            index: &self.index,
            next_seq: &self.next_seq,
        }
    }
}
