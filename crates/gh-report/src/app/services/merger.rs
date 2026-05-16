//! `Merger` — single-task command merger that holds the sole
//! [`EventStore`] write handle (Phase 2 v2 Track 4.0,
//! `adr-fmt-nnn3`).
//!
//! Closes the I1 TOCTOU window (lookup-then-create on the routing
//! index across the three ApplicationServices, see
//! [`super::shared`] module docs) by collapsing all command-driven
//! writes into one [`tokio::task`] consuming a single
//! [`mpsc::channel`]. The three [`super::run_service::RunService`],
//! [`super::repo_service::RepoService`],
//! [`super::webhook_service::WebhookService`] surfaces become thin
//! channel-send wrappers in Tracks 4.0/3b → 4.0/5; the Merger arms
//! contain the load → handle → create-or-append → publish triad
//! lifted verbatim from those services (same [`Arc`] clones, same
//! [`super::shared`] helpers — relocated, not rewritten).
//!
//! ## Reachability at Track 4.0/3a
//!
//! The Merger task is spawned and held by
//! [`AppState`](crate::app::state::AppState), but **no caller routes
//! through `merger_tx` yet**: every production publish site continues
//! to call the existing service methods directly. The arms compile,
//! the task receives `cmd` messages only from tests that exercise the
//! Merger surface (none in 3a). This is the structural Tidy First
//! framing — at 3b the `RunService` public methods become
//! `merger_tx.send(...).await` wrappers and the arms here become
//! load-bearing; replay-equivalence (Track 4.0 success criterion #4)
//! holds trivially because the on-the-wire envelope sequence is
//! produced by the same shared helpers in both worlds.
//!
//! ## Why `tokio::sync::mpsc` + `oneshot` reply
//!
//! Each `MergerCommand` variant carries a [`oneshot::Sender`] reply
//! channel so the caller's existing `.await? -> Result<(), …Error>`
//! semantics are preserved verbatim — call-site signatures at 3b/4/5
//! stay byte-identical at the suspension-point boundary. The reply
//! type is the matching service error per CHE-0054:R4
//! (`RunError`/`RepoError`/`WebhookError` — see Track 4.0 brief
//! "ApplicationService public APIs become thin channel-send
//! wrappers").
//!
//! ## Why module-private to `services/`
//!
//! Placed inside `app/services/` so the
//! [`pub(super)`](super::shared) helpers in
//! [`super::shared`] remain reachable without widening visibility.
//! Re-exported through [`super`] at the `mod.rs` level so external
//! call-sites (`app::state`, `app::collect`, `webhook::*`,
//! `infra::server::server`) consume `Merger` / `MergerCommand` via
//! `crate::app::services::{Merger, MergerCommand}` from 3b onward.

use std::collections::HashMap;
use std::num::NonZeroU64;
use std::sync::{Arc, Mutex};

use cherry_pit_agent::InProcessEventBus;
use cherry_pit_core::{AggregateId, CorrelationContext};
use cherry_pit_gateway::MsgpackFileStore;
use tokio::sync::{mpsc, oneshot};

use crate::domain::aggregates::repo::{RecordEvaluation, RecordRemoval, RepoError};
use crate::domain::aggregates::run::{
    CompleteSweep, FailSweep, PublishEvidence, RecordProgress, RunError, StartSweep,
};
use crate::domain::aggregates::webhook::{RecordDelivery, WebhookError};
use crate::domain::events::DomainEvent;

/// Channel capacity for the Merger command queue.
///
/// Sized large enough that bursty webhook ingestion + scheduled-sweep
/// progress checkpoints do not back-pressure call sites under normal
/// load. A full queue blocks the sender on `mpsc::Sender::send`; the
/// caller still observes its `.await` semantics, so a brief stall is
/// preferable to a `try_send` failure path. Revisit if the post-3b
/// telemetry shows sustained queue depth.
const MERGER_CHANNEL_CAPACITY: usize = 1024;

/// Concrete monomorphisation of the durable per-aggregate event
/// store wired into [`AppState`](crate::app::state::AppState).
///
/// Bound here (rather than carrying generic `<S, B>` on
/// [`MergerCommand`]) because the command enum crosses the channel
/// boundary and would otherwise force the channel and every call
/// site to thread the same two type parameters. The Merger is the
/// composition-root sink — there is exactly one concrete pair in
/// gh-report (CHE-0005:R1 + CHE-0054 §"Open γ" resolution at Inc
/// B7'a-6) — so binding the types at the Merger surface is
/// type-safe and ergonomic.
type Store = MsgpackFileStore<DomainEvent>;
/// Concrete monomorphisation of the in-process bus. See [`Store`].
type Bus = InProcessEventBus<DomainEvent>;

/// Commands routed through the [`Merger`] task.
///
/// One variant per ApplicationService public method (eight total,
/// mirroring the five [`super::run_service::RunService`] methods,
/// the two [`super::repo_service::RepoService`] methods, and the
/// single [`super::webhook_service::WebhookService::ingest`] surface).
/// Each variant carries its existing command struct plus the routing
/// key the corresponding service method takes today (`batch_id` for
/// Run append-path; `domain_key` for Repo; none for Run create-path
/// and Webhook ingest) plus a [`oneshot::Sender`] reply with the
/// matching error type.
///
/// The variants are documented inline with the lifted service method
/// they mirror, so a reviewer can compare the Merger arm body in
/// [`Merger::run`] against the corresponding service source.
#[derive(Debug)]
pub enum MergerCommand {
    /// Mirrors [`super::run_service::RunService::start_sweep`] —
    /// create-path for the [`Run`] aggregate. Routing key
    /// (`batch_id`) is carried inline on [`StartSweep`].
    ///
    /// [`Run`]: crate::domain::aggregates::run::Run
    StartSweep {
        cmd: StartSweep,
        ctx: CorrelationContext,
        reply: oneshot::Sender<Result<(), RunError>>,
    },
    /// Mirrors [`super::run_service::RunService::record_progress`] —
    /// append-path for the [`Run`] aggregate. `batch_id` is the
    /// routing key (CHE-0054:R5).
    ///
    /// [`Run`]: crate::domain::aggregates::run::Run
    RecordProgress {
        batch_id: String,
        cmd: RecordProgress,
        ctx: CorrelationContext,
        reply: oneshot::Sender<Result<(), RunError>>,
    },
    /// Mirrors [`super::run_service::RunService::complete`] —
    /// success-terminal for the [`Run`] aggregate.
    ///
    /// [`Run`]: crate::domain::aggregates::run::Run
    CompleteSweep {
        batch_id: String,
        cmd: CompleteSweep,
        ctx: CorrelationContext,
        reply: oneshot::Sender<Result<(), RunError>>,
    },
    /// Mirrors [`super::run_service::RunService::fail`] —
    /// failure-terminal for the [`Run`] aggregate.
    ///
    /// [`Run`]: crate::domain::aggregates::run::Run
    FailSweep {
        batch_id: String,
        cmd: FailSweep,
        ctx: CorrelationContext,
        reply: oneshot::Sender<Result<(), RunError>>,
    },
    /// Mirrors [`super::run_service::RunService::publish_evidence`] —
    /// post-completion evidence publish for the [`Run`] aggregate.
    ///
    /// [`Run`]: crate::domain::aggregates::run::Run
    PublishEvidence {
        batch_id: String,
        cmd: PublishEvidence,
        ctx: CorrelationContext,
        reply: oneshot::Sender<Result<(), RunError>>,
    },
    /// Mirrors [`super::repo_service::RepoService::record_evaluation`]
    /// — lazy-create-or-append for the [`Repo`] aggregate.
    /// `domain_key` is the routing key (CHE-0054:R5).
    ///
    /// [`Repo`]: crate::domain::aggregates::repo::Repo
    RecordEvaluation {
        domain_key: String,
        cmd: Box<RecordEvaluation>,
        ctx: CorrelationContext,
        reply: oneshot::Sender<Result<(), RepoError>>,
    },
    /// Mirrors [`super::repo_service::RepoService::record_removal`] —
    /// lazy-create-or-append-then-terminal for the [`Repo`] aggregate.
    ///
    /// [`Repo`]: crate::domain::aggregates::repo::Repo
    RecordRemoval {
        domain_key: String,
        cmd: RecordRemoval,
        ctx: CorrelationContext,
        reply: oneshot::Sender<Result<(), RepoError>>,
    },
    /// Mirrors [`super::webhook_service::WebhookService::ingest`] —
    /// fresh-per-delivery create-path for the [`WebhookDelivery`]
    /// aggregate. Routing key (`delivery_id`) is carried inline on
    /// [`RecordDelivery`].
    ///
    /// [`WebhookDelivery`]: crate::domain::aggregates::webhook::WebhookDelivery
    IngestWebhook {
        cmd: RecordDelivery,
        ctx: CorrelationContext,
        reply: oneshot::Sender<Result<(), WebhookError>>,
    },
}

/// Single-task command merger holding the sole [`EventStore`] write
/// handle (Track 4.0/3a scaffold).
///
/// Owns [`Arc`] clones of the same handles each ApplicationService
/// holds today — the per-aggregate event store, the in-process bus,
/// the three routing indices, and the shared sequence tracker. The
/// task body matches on incoming [`MergerCommand`] variants and runs
/// the load → handle → create-or-append → publish triad lifted
/// verbatim from the matching service method.
///
/// At Track 4.0/3a the task is reachable only through
/// [`Self::spawn`]'s returned [`mpsc::Sender`]; no production caller
/// routes through it. Track 4.0/3b/4/5 switch each call site from
/// `service.method(...)` to `merger_tx.send(MergerCommand::...).await`;
/// Track 4.0/6 deletes the now-dead ApplicationService write logic.
///
/// [`EventStore`]: cherry_pit_core::EventStore
pub struct Merger {
    store: Arc<Store>,
    bus: Arc<Bus>,
    run_index: Arc<Mutex<HashMap<String, AggregateId>>>,
    repo_index: Arc<Mutex<HashMap<String, AggregateId>>>,
    delivery_index: Arc<Mutex<HashMap<String, AggregateId>>>,
    sequence_tracker: Arc<Mutex<HashMap<AggregateId, NonZeroU64>>>,
}

impl Merger {
    /// Spawn the Merger task and return the producer end of its
    /// command channel plus the [`tokio::task::JoinHandle`].
    ///
    /// The handle is held by [`AppState`](crate::app::state::AppState)
    /// to keep the task alive for the process lifetime; it is not
    /// joined explicitly (process exit drops it). The channel is
    /// bounded ([`MERGER_CHANNEL_CAPACITY`]) so a saturated queue
    /// back-pressures producers via `mpsc::Sender::send` rather than
    /// dropping commands.
    #[must_use]
    pub fn spawn(
        store: Arc<Store>,
        bus: Arc<Bus>,
        run_index: Arc<Mutex<HashMap<String, AggregateId>>>,
        repo_index: Arc<Mutex<HashMap<String, AggregateId>>>,
        delivery_index: Arc<Mutex<HashMap<String, AggregateId>>>,
        sequence_tracker: Arc<Mutex<HashMap<AggregateId, NonZeroU64>>>,
    ) -> (mpsc::Sender<MergerCommand>, tokio::task::JoinHandle<()>) {
        let merger = Self {
            store,
            bus,
            run_index,
            repo_index,
            delivery_index,
            sequence_tracker,
        };
        let (tx, rx) = mpsc::channel(MERGER_CHANNEL_CAPACITY);
        let handle = tokio::spawn(merger.run(rx));
        (tx, handle)
    }

    /// Main task loop: receive commands and dispatch to the lifted
    /// service triad bodies.
    ///
    /// Channel close (every [`mpsc::Sender`] dropped — i.e. process
    /// shutdown after [`AppState`](crate::app::state::AppState) drops)
    /// exits the loop cleanly. Reply-side `oneshot::Sender::send`
    /// failure is swallowed (caller dropped the receiver) — the
    /// persistence + publish work already completed and the error is
    /// informational at that point.
    async fn run(self, mut rx: mpsc::Receiver<MergerCommand>) {
        while let Some(cmd) = rx.recv().await {
            match cmd {
                MergerCommand::StartSweep { cmd, ctx, reply } => {
                    let result = self.handle_start_sweep(cmd, &ctx).await;
                    let _ = reply.send(result);
                }
                MergerCommand::RecordProgress {
                    batch_id,
                    cmd,
                    ctx,
                    reply,
                } => {
                    let result = self.handle_record_progress(&batch_id, cmd, &ctx).await;
                    let _ = reply.send(result);
                }
                MergerCommand::CompleteSweep {
                    batch_id,
                    cmd,
                    ctx,
                    reply,
                } => {
                    let result = self.handle_complete_sweep(&batch_id, cmd, &ctx).await;
                    let _ = reply.send(result);
                }
                MergerCommand::FailSweep {
                    batch_id,
                    cmd,
                    ctx,
                    reply,
                } => {
                    let result = self.handle_fail_sweep(&batch_id, cmd, &ctx).await;
                    let _ = reply.send(result);
                }
                MergerCommand::PublishEvidence {
                    batch_id,
                    cmd,
                    ctx,
                    reply,
                } => {
                    let result = self.handle_publish_evidence(&batch_id, cmd, &ctx).await;
                    let _ = reply.send(result);
                }
                MergerCommand::RecordEvaluation {
                    domain_key,
                    cmd,
                    ctx,
                    reply,
                } => {
                    let result = self.handle_record_evaluation(&domain_key, *cmd, &ctx).await;
                    let _ = reply.send(result);
                }
                MergerCommand::RecordRemoval {
                    domain_key,
                    cmd,
                    ctx,
                    reply,
                } => {
                    let result = self.handle_record_removal(&domain_key, cmd, &ctx).await;
                    let _ = reply.send(result);
                }
                MergerCommand::IngestWebhook { cmd, ctx, reply } => {
                    let result = self.handle_ingest_webhook(cmd, &ctx).await;
                    let _ = reply.send(result);
                }
            }
        }
    }

    // ── Run aggregate arms (lifted from RunService) ──────────────────

    /// Lifted from [`super::run_service::RunService::start_sweep`].
    async fn handle_start_sweep(
        &self,
        cmd: StartSweep,
        ctx: &CorrelationContext,
    ) -> Result<(), RunError> {
        use cherry_pit_core::{Aggregate, HandleCommand};

        let domain_key = cmd.batch_id.clone();
        let existing_id = super::shared::lookup(&self.run_index, &domain_key);
        let (envelopes, last_seq) =
            super::shared::load_envelopes_or_empty(&self.store, existing_id).await;
        let mut state = crate::domain::aggregates::run::Run::default();
        for env in &envelopes {
            state.apply(env.payload());
        }
        let new_events = state.handle(cmd)?;
        let new_envelopes = super::shared::create_or_append(
            super::shared::PersistHandles {
                store: &self.store,
                index: &self.run_index,
                sequence_tracker: &self.sequence_tracker,
            },
            &domain_key,
            existing_id,
            last_seq,
            new_events,
            ctx,
        )
        .await;
        super::shared::publish_or_trace(&self.bus, &new_envelopes, "SweepStarted").await;
        Ok(())
    }

    /// Lifted from [`super::run_service::RunService::record_progress`].
    async fn handle_record_progress(
        &self,
        batch_id: &str,
        cmd: RecordProgress,
        ctx: &CorrelationContext,
    ) -> Result<(), RunError> {
        use cherry_pit_core::HandleCommand;

        let id = self.resolve_run_id(batch_id)?;
        let (state, last_seq) = self.load_and_fold_run(id).await;
        let new_events = state.handle(cmd)?;
        let new_envelopes = self.append_and_track(id, last_seq, new_events, ctx).await;
        super::shared::publish_or_trace(&self.bus, &new_envelopes, "SweepProgress").await;
        Ok(())
    }

    /// Lifted from [`super::run_service::RunService::complete`].
    async fn handle_complete_sweep(
        &self,
        batch_id: &str,
        cmd: CompleteSweep,
        ctx: &CorrelationContext,
    ) -> Result<(), RunError> {
        use cherry_pit_core::HandleCommand;

        let id = self.resolve_run_id(batch_id)?;
        let (state, last_seq) = self.load_and_fold_run(id).await;
        let new_events = state.handle(cmd)?;
        let new_envelopes = self.append_and_track(id, last_seq, new_events, ctx).await;
        super::shared::publish_or_trace(&self.bus, &new_envelopes, "SweepCompleted").await;
        Ok(())
    }

    /// Lifted from [`super::run_service::RunService::fail`].
    async fn handle_fail_sweep(
        &self,
        batch_id: &str,
        cmd: FailSweep,
        ctx: &CorrelationContext,
    ) -> Result<(), RunError> {
        use cherry_pit_core::HandleCommand;

        let id = self.resolve_run_id(batch_id)?;
        let (state, last_seq) = self.load_and_fold_run(id).await;
        let new_events = state.handle(cmd)?;
        let new_envelopes = self.append_and_track(id, last_seq, new_events, ctx).await;
        super::shared::publish_or_trace(&self.bus, &new_envelopes, "SweepFailed").await;
        Ok(())
    }

    /// Lifted from [`super::run_service::RunService::publish_evidence`].
    async fn handle_publish_evidence(
        &self,
        batch_id: &str,
        cmd: PublishEvidence,
        ctx: &CorrelationContext,
    ) -> Result<(), RunError> {
        use cherry_pit_core::HandleCommand;

        let id = self.resolve_run_id(batch_id)?;
        let (state, last_seq) = self.load_and_fold_run(id).await;
        let new_events = state.handle(cmd)?;
        let new_envelopes = self.append_and_track(id, last_seq, new_events, ctx).await;
        super::shared::publish_or_trace(&self.bus, &new_envelopes, "EvidencePublished").await;
        Ok(())
    }

    // ── Repo aggregate arms (lifted from RepoService) ────────────────

    /// Lifted from [`super::repo_service::RepoService::record_evaluation`].
    async fn handle_record_evaluation(
        &self,
        domain_key: &str,
        cmd: RecordEvaluation,
        ctx: &CorrelationContext,
    ) -> Result<(), RepoError> {
        use cherry_pit_core::{Aggregate, HandleCommand};

        let existing_id = super::shared::lookup(&self.repo_index, domain_key);
        let (envelopes, last_seq) =
            super::shared::load_envelopes_or_empty(&self.store, existing_id).await;
        let mut state = crate::domain::aggregates::repo::Repo::default();
        for env in &envelopes {
            state.apply(env.payload());
        }
        let new_events = state.handle(cmd)?;
        let new_envelopes = super::shared::create_or_append(
            super::shared::PersistHandles {
                store: &self.store,
                index: &self.repo_index,
                sequence_tracker: &self.sequence_tracker,
            },
            domain_key,
            existing_id,
            last_seq,
            new_events,
            ctx,
        )
        .await;
        super::shared::publish_or_trace(&self.bus, &new_envelopes, "RepoEvaluated").await;
        Ok(())
    }

    /// Lifted from [`super::repo_service::RepoService::record_removal`].
    async fn handle_record_removal(
        &self,
        domain_key: &str,
        cmd: RecordRemoval,
        ctx: &CorrelationContext,
    ) -> Result<(), RepoError> {
        use cherry_pit_core::{Aggregate, HandleCommand};

        let existing_id = super::shared::lookup(&self.repo_index, domain_key);
        let (envelopes, last_seq) =
            super::shared::load_envelopes_or_empty(&self.store, existing_id).await;
        let mut state = crate::domain::aggregates::repo::Repo::default();
        for env in &envelopes {
            state.apply(env.payload());
        }
        let new_events = state.handle(cmd)?;
        let new_envelopes = super::shared::create_or_append(
            super::shared::PersistHandles {
                store: &self.store,
                index: &self.repo_index,
                sequence_tracker: &self.sequence_tracker,
            },
            domain_key,
            existing_id,
            last_seq,
            new_events,
            ctx,
        )
        .await;
        super::shared::publish_or_trace(&self.bus, &new_envelopes, "RepoRemoved").await;
        Ok(())
    }

    // ── Webhook aggregate arm (lifted from WebhookService) ───────────

    /// Lifted from [`super::webhook_service::WebhookService::ingest`].
    async fn handle_ingest_webhook(
        &self,
        cmd: RecordDelivery,
        ctx: &CorrelationContext,
    ) -> Result<(), WebhookError> {
        use cherry_pit_core::{EventStore, HandleCommand};

        let delivery_id = cmd.delivery_id.clone();
        let state = crate::domain::aggregates::webhook::WebhookDelivery::default();
        let new_events = state.handle(cmd)?;
        let (assigned_id, new_envelopes) = self
            .store
            .create(new_events, ctx.clone())
            .await
            .expect("EventStore::create failure path enriched in B7'c");
        {
            let mut guard = self
                .delivery_index
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard.entry(delivery_id).or_insert(assigned_id);
        }
        if let Some(env) = new_envelopes.last() {
            let seq = env.sequence();
            let mut guard = self
                .sequence_tracker
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard.insert(assigned_id, seq);
        }
        super::shared::publish_or_trace(&self.bus, &new_envelopes, "WebhookReceived").await;
        Ok(())
    }

    // ── Private append-path helpers (lifted from RunService) ─────────

    /// Lifted from [`super::run_service::RunService::resolve_id`].
    /// Specialised to the Run routing index; Repo uses the
    /// `lookup`/`create_or_append` lazy-create path so does not need
    /// the strict resolver.
    fn resolve_run_id(&self, batch_id: &str) -> Result<AggregateId, RunError> {
        let guard = self
            .run_index
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard
            .get(batch_id)
            .copied()
            .ok_or_else(|| RunError::RoutingMiss(batch_id.into()))
    }

    /// Lifted from [`super::run_service::RunService::load_and_fold`].
    async fn load_and_fold_run(
        &self,
        id: AggregateId,
    ) -> (crate::domain::aggregates::run::Run, NonZeroU64) {
        use cherry_pit_core::{Aggregate, EventStore};

        let envelopes = self
            .store
            .load(id)
            .await
            .expect("EventStore::load failure path enriched in B7'c");
        let mut state = crate::domain::aggregates::run::Run::default();
        for env in &envelopes {
            state.apply(env.payload());
        }
        let last_seq = envelopes
            .last()
            .map(cherry_pit_core::EventEnvelope::sequence)
            .expect("indexed AggregateId must have ≥1 envelope (corrupt routing otherwise)");
        (state, last_seq)
    }

    /// Lifted from [`super::run_service::RunService::append_and_track`].
    async fn append_and_track(
        &self,
        id: AggregateId,
        last_seq: NonZeroU64,
        new_events: Vec<DomainEvent>,
        ctx: &CorrelationContext,
    ) -> Vec<cherry_pit_core::EventEnvelope<DomainEvent>> {
        use cherry_pit_core::EventStore;

        let new_envelopes = self
            .store
            .append(id, last_seq, new_events, ctx.clone())
            .await
            .expect("EventStore::append failure path enriched in B7'c");
        if let Some(env) = new_envelopes.last() {
            let next = env.sequence();
            let mut guard = self
                .sequence_tracker
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard.insert(id, next);
        }
        new_envelopes
    }
}
