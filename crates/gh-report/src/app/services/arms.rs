//! Per-aggregate [`MergerArm`] impls — three thin adapters that
//! consume the lifted [`cherry_pit_merger::Merger`] primitive
//! (CHE-0069). Mission H, bd `adr-fmt-cq7vb.11`.
//!
//! Each arm pairs one of gh-report's three aggregates with the
//! corresponding [`PersistMode`] variant:
//!
//! | Aggregate          | Arm           | [`PersistMode`]                     | Notes                                          |
//! |--------------------|---------------|-------------------------------------|------------------------------------------------|
//! | [`Run`]            | [`RunArm`]    | `Create` for `StartSweep`; `AppendStrict(batch_id)` for the rest | CHE-0054:R1.a/R1.b/R1.c/R1.d/R1.e ordering    |
//! | [`Repo`]           | [`RepoArm`]   | `CreateOrAppend(domain_key)`        | CHE-0054:R2 — lazy-create-or-append            |
//! | [`WebhookDelivery`]| [`WebhookArm`]| `Create`                            | CHE-0054:R3 — fresh-per-delivery, no indexing  |
//!
//! Each arm's command type is a per-aggregate gh-report-internal
//! enum (`RunCmd`/`RepoCmd`/`WebhookCmd`) carrying the existing
//! [`StartSweep`]/[`RecordEvaluation`]/[`RecordDelivery`] commands
//! alongside the routing key the previous in-crate merger required.
//! Per-service public methods (e.g. [`RunService::start_sweep`])
//! stay byte-identical at the call-site boundary per CHE-0054:R10 —
//! the cmd enum lives strictly behind the
//! [`MergerHandle::dispatch`] surface.
//!
//! ## Webhook indexing: dropped, deliberately
//!
//! The original in-crate [`Merger::handle_ingest_webhook`] populated
//! `AppState::deliveries_by_id` after the fresh `EventStore::create`
//! via `entry().or_insert()`. That index entry had **zero production
//! readers** (verified by `rg deliveries_by_id` — only writers in the
//! merger arm; `bootstrap_replay` docs at `state.rs:743-754` explicitly
//! note the index cannot be rebuilt from events and is empty in
//! steady state). Lifting onto [`cherry_pit_merger`]'s
//! [`PersistMode::Create`] — which by design does **not** touch a
//! routing index — drops the vestigial write. The `deliveries_by_id`
//! handle remains on [`AppState`] for now (downstream consumers may
//! materialise it via a different mechanism later); the affected
//! webhook-service tests assert against `EventStore::list_aggregates`
//! rather than the index entry. See also CHE-0069:R3 (the three
//! [`PersistMode`] variants are bounded; adding a fourth shape
//! requires a superseding ADR).
//!
//! [`Run`]: crate::domain::aggregates::run::Run
//! [`Repo`]: crate::domain::aggregates::repo::Repo
//! [`WebhookDelivery`]: crate::domain::aggregates::webhook::WebhookDelivery
//! [`StartSweep`]: crate::domain::aggregates::run::StartSweep
//! [`RecordEvaluation`]: crate::domain::aggregates::repo::RecordEvaluation
//! [`RecordDelivery`]: crate::domain::aggregates::webhook::RecordDelivery
//! [`Merger::handle_ingest_webhook`]: super::merger::Merger
//! [`AppState`]: crate::app::state::AppState
//! [`RunService::start_sweep`]: super::run_service::RunService::start_sweep
//! [`MergerHandle::dispatch`]: cherry_pit_merger::MergerHandle::dispatch
//! [`PersistMode`]: cherry_pit_merger::PersistMode
//! [`PersistMode::Create`]: cherry_pit_merger::PersistMode::Create
//! [`cherry_pit_merger`]: cherry_pit_merger

use cherry_pit_core::{Aggregate, HandleCommand};
use cherry_pit_merger::{MergerArm, PersistMode};

use crate::domain::aggregates::repo::{RecordEvaluation, RecordRemoval, Repo, RepoError};
use crate::domain::aggregates::run::{
    CompleteSweep, FailSweep, PublishEvidence, RecordProgress, RenderPartial, Run, RunError,
    StartSweep,
};
use crate::domain::aggregates::webhook::{RecordDelivery, WebhookDelivery, WebhookError};

/// Channel payload for the [`Run`](crate::domain::aggregates::run::Run)
/// merger arm. Six variants — one per gh-report `RunService` public
/// method (CHE-0054:R1).
///
/// `StartSweep` uses [`PersistMode::Create`] (well, `CreateOrAppend`
/// — see [`RunArm::persist_mode`]); the remaining five use
/// [`PersistMode::AppendStrict`] with the carried `batch_id` per
/// CHE-0054:R5.
#[derive(Debug)]
pub enum RunCmd {
    /// [`RunService::start_sweep`](super::run_service::RunService::start_sweep)
    /// — create-path for the Run aggregate. The `batch_id` lives on
    /// the [`StartSweep`] payload itself.
    Start(StartSweep),
    /// [`RunService::record_progress`](super::run_service::RunService::record_progress).
    Progress {
        batch_id: String,
        cmd: RecordProgress,
    },
    /// [`RunService::complete`](super::run_service::RunService::complete).
    Complete {
        batch_id: String,
        cmd: CompleteSweep,
    },
    /// [`RunService::fail`](super::run_service::RunService::fail).
    Fail { batch_id: String, cmd: FailSweep },
    /// [`RunService::publish_evidence`](super::run_service::RunService::publish_evidence).
    Publish {
        batch_id: String,
        cmd: PublishEvidence,
    },
    /// [`RunService::render_partial`](super::run_service::RunService::render_partial).
    Partial {
        batch_id: String,
        cmd: RenderPartial,
    },
}

/// Channel payload for the [`Repo`](crate::domain::aggregates::repo::Repo)
/// merger arm. Two variants — one per gh-report `RepoService` public
/// method (CHE-0054:R2). Both use
/// [`PersistMode::CreateOrAppend`] keyed by `domain_key`.
#[derive(Debug)]
pub enum RepoCmd {
    /// [`RepoService::record_evaluation`](super::repo_service::RepoService::record_evaluation).
    /// `Box` matches the original [`MergerCommand`] shape to keep
    /// the enum size budget unchanged.
    ///
    /// [`MergerCommand`]: super::merger::MergerCommand
    Evaluate {
        domain_key: String,
        cmd: Box<RecordEvaluation>,
    },
    /// [`RepoService::record_removal`](super::repo_service::RepoService::record_removal).
    Remove {
        domain_key: String,
        cmd: RecordRemoval,
    },
}

/// Channel payload for the
/// [`WebhookDelivery`](crate::domain::aggregates::webhook::WebhookDelivery)
/// merger arm. One variant; uses [`PersistMode::Create`] — fresh
/// aggregate per call, no routing-index update.
#[derive(Debug)]
pub enum WebhookCmd {
    /// [`WebhookService::ingest`](super::webhook_service::WebhookService::ingest).
    Ingest(RecordDelivery),
}

/// [`MergerArm`] impl for the Run aggregate.
///
/// `StartSweep` is the only create-path variant. The original in-crate
/// merger routed `StartSweep` through the `lookup → create-or-append`
/// helper (which means a same-`batch_id` second `StartSweep` would
/// `append` to the existing aggregate, surfacing
/// [`RunError::AlreadyStarted`] from the aggregate's
/// [`HandleCommand::handle`] guard). The lifted primitive's
/// [`PersistMode::CreateOrAppend`] preserves that exact behaviour;
/// [`PersistMode::Create`] would instead orphan a fresh aggregate per
/// retry, losing the dedup property the routing-index lookup gave us.
/// `StartSweep` therefore uses `CreateOrAppend(batch_id)`, not
/// `Create`.
#[derive(Debug, Default)]
pub struct RunArm;

impl MergerArm<Run> for RunArm {
    type Cmd = RunCmd;
    type Err = RunError;

    fn persist_mode(&self, cmd: &Self::Cmd) -> PersistMode {
        match cmd {
            RunCmd::Start(start) => PersistMode::CreateOrAppend(start.batch_id.clone()),
            RunCmd::Progress { batch_id, .. }
            | RunCmd::Complete { batch_id, .. }
            | RunCmd::Fail { batch_id, .. }
            | RunCmd::Publish { batch_id, .. }
            | RunCmd::Partial { batch_id, .. } => PersistMode::AppendStrict(batch_id.clone()),
        }
    }

    fn handle(
        &self,
        state: &Run,
        cmd: Self::Cmd,
    ) -> Result<Vec<<Run as Aggregate>::Event>, Self::Err> {
        match cmd {
            RunCmd::Start(start) => state.handle(start),
            RunCmd::Progress { cmd, .. } => state.handle(cmd),
            RunCmd::Complete { cmd, .. } => state.handle(cmd),
            RunCmd::Fail { cmd, .. } => state.handle(cmd),
            RunCmd::Publish { cmd, .. } => state.handle(cmd),
            RunCmd::Partial { cmd, .. } => state.handle(cmd),
        }
    }

    fn publish_label(&self, cmd: &Self::Cmd) -> &'static str {
        match cmd {
            RunCmd::Start(_) => "SweepStarted",
            RunCmd::Progress { .. } => "SweepProgress",
            RunCmd::Complete { .. } => "SweepCompleted",
            RunCmd::Fail { .. } => "SweepFailed",
            RunCmd::Publish { .. } => "EvidencePublished",
            RunCmd::Partial { .. } => "PartialEvidenceRendered",
        }
    }

    fn missing_key_error(&self, key: &str) -> Self::Err {
        RunError::RoutingMiss(key.to_owned())
    }
}

/// [`MergerArm`] impl for the Repo aggregate.
///
/// Both variants are `CreateOrAppend` — the in-crate merger's
/// `handle_record_evaluation` / `handle_record_removal` both used the
/// `lookup → load → handle → create_or_append → publish` triad, and
/// CHE-0054:R2 explicitly permits `RecordRemoval` on a never-evaluated
/// repo (webhook-driven removal preceding local eval). The lazy
/// create path materialises a fresh `Repo` aggregate on first
/// reference; subsequent same-`domain_key` references append.
#[derive(Debug, Default)]
pub struct RepoArm;

impl MergerArm<Repo> for RepoArm {
    type Cmd = RepoCmd;
    type Err = RepoError;

    fn persist_mode(&self, cmd: &Self::Cmd) -> PersistMode {
        match cmd {
            RepoCmd::Evaluate { domain_key, .. } | RepoCmd::Remove { domain_key, .. } => {
                PersistMode::CreateOrAppend(domain_key.clone())
            }
        }
    }

    fn handle(
        &self,
        state: &Repo,
        cmd: Self::Cmd,
    ) -> Result<Vec<<Repo as Aggregate>::Event>, Self::Err> {
        match cmd {
            RepoCmd::Evaluate { cmd, .. } => state.handle(*cmd),
            RepoCmd::Remove { cmd, .. } => state.handle(cmd),
        }
    }

    fn publish_label(&self, cmd: &Self::Cmd) -> &'static str {
        match cmd {
            RepoCmd::Evaluate { .. } => "RepoEvaluated",
            RepoCmd::Remove { .. } => "RepoRemoved",
        }
    }
}

/// [`MergerArm`] impl for the
/// [`WebhookDelivery`](crate::domain::aggregates::webhook::WebhookDelivery)
/// aggregate.
///
/// Pure [`PersistMode::Create`] — every ingest mints a fresh
/// aggregate. No routing-index update (see module docs). The
/// [`WebhookDelivery`](crate::domain::aggregates::webhook::WebhookDelivery)
/// aggregate is degenerate-terminal per CHE-0054:R3; idempotency
/// against duplicate `delivery_id`s lives at the call-site
/// (`webhook/mod.rs::seen_deliveries` cache).
#[derive(Debug, Default)]
pub struct WebhookArm;

impl MergerArm<WebhookDelivery> for WebhookArm {
    type Cmd = WebhookCmd;
    type Err = WebhookError;

    fn persist_mode(&self, _cmd: &Self::Cmd) -> PersistMode {
        PersistMode::Create
    }

    fn handle(
        &self,
        state: &WebhookDelivery,
        cmd: Self::Cmd,
    ) -> Result<Vec<<WebhookDelivery as Aggregate>::Event>, Self::Err> {
        match cmd {
            WebhookCmd::Ingest(rec) => state.handle(rec),
        }
    }

    fn publish_label(&self, _cmd: &Self::Cmd) -> &'static str {
        "WebhookReceived"
    }
}
