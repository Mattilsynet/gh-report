//! M2.a' — Proves the bus-publish failure absorb point emits a
//! structured `tracing::error!` per envelope with the five required
//! fields (`event_id`, `correlation_id`, `causation_id`,
//! `aggregate_id`, `error`) plus the existing `event` label.
//!
//! Cites: CHE-0024:R1 (publish failure non-fatal — events persisted),
//!        CHE-0024:R3 (consumers reconcile via replay from `EventStore::load`),
//!        COM-0019:R1 (structured emission at absorb point),
//!        COM-0019:R4 (`correlation_id` flows through observability boundary),
//!        COM-0019:R7 (`EventBus` retry-absorb telemetry — `error!` severity).
//!
//! Test design (Track 4.0/3b, mission `adr-fmt-nnn3`):
//! - The `publish_or_trace` absorb point now lives inside the
//!   [`Merger`] task — the bus moved off `RunService` (which became a
//!   thin `merger_tx.send(...).await` wrapper) and into the Merger
//!   per Track 4.0/3b. The matching failure-injection seam is
//!   `Merger::with_bus_for_test`, the canonical test-only ctor added
//!   for this class of test (shared by step-4 / step-5 reroute tests;
//!   not re-litigated per-step).
//! - Wire a real `PardosaFileEventStore` (persistence still succeeds —
//!   CHE-0024:R1) and a `FailingBus` that returns `Err(BusError)`
//!   from `publish`. Spawn the Merger over both via
//!   `with_bus_for_test`; drive a single `MergerCommand::StartSweep`
//!   through the channel and await the reply.
//! - Capture emissions via a `tracing_subscriber::Layer` that records
//!   each event's field values into a shared `Vec`.
//! - Assert exactly one captured ERROR event with all five required
//!   fields on the `gh_report.eda` target (one envelope ⇒ one
//!   per-envelope emission).

use std::collections::HashMap;
use std::num::NonZeroU64;
use std::sync::{Arc, Mutex};

use cherry_pit_core::testing::InMemoryEventStore;
use cherry_pit_core::{AggregateId, BusError, CorrelationContext, EventBus, EventEnvelope};
use tempfile::TempDir;
use tokio::sync::oneshot;
use tracing::Subscriber;
use tracing::field::{Field, Visit};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::{Context, SubscriberExt};

use gh_report::app::services::{Merger, MergerCommand};
use gh_report::domain::aggregates::run::StartSweep;
use gh_report::domain::events::DomainEvent;

/// Fake `EventBus` that always returns `Err(BusError)` from `publish`.
/// Drives the `publish_or_trace` helper down its error arm.
#[derive(Default)]
struct FailingBus;

impl EventBus for FailingBus {
    type Event = DomainEvent;

    async fn publish(&self, _events: &[EventEnvelope<Self::Event>]) -> Result<(), BusError> {
        Err(BusError::new("simulated bus failure for M2.a' test"))
    }
}

/// Captured tracing event: target + level + every field rendered as
/// its `Debug` (or string) form. Field-name presence is what the
/// assertions check; values are kept for diagnostic dumps on failure.
#[derive(Debug, Clone)]
struct CapturedEvent {
    target: String,
    level: String,
    fields: HashMap<String, String>,
}

struct CaptureVisitor<'a> {
    fields: &'a mut HashMap<String, String>,
}

impl Visit for CaptureVisitor<'_> {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.fields
            .insert(field.name().to_string(), format!("{value:?}"));
    }
    fn record_str(&mut self, field: &Field, value: &str) {
        self.fields
            .insert(field.name().to_string(), value.to_string());
    }
}

#[derive(Clone, Default)]
struct CaptureLayer {
    events: Arc<Mutex<Vec<CapturedEvent>>>,
}

impl<S: Subscriber> Layer<S> for CaptureLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        let mut fields = HashMap::new();
        let mut visitor = CaptureVisitor {
            fields: &mut fields,
        };
        event.record(&mut visitor);
        let captured = CapturedEvent {
            target: event.metadata().target().to_string(),
            level: event.metadata().level().to_string(),
            fields,
        };
        self.events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(captured);
    }
}

#[tokio::test]
async fn publish_failure_emits_structured_error_per_envelope() {
    // Persistence: interim InMemoryEventStore (publish failure is non-fatal
    // per CHE-0024:R1; events still durable in-process for this test's
    // scope. Interim substrate; see follow-up to mission
    // cherry-pit-pardosa-deletion-1779215265 for the PGNO-backed successor).
    let dir = TempDir::new().expect("tempdir");
    let _ = dir.path(); // (was: PardosaFileEventStore::<DomainEvent>::open(dir.path()); see follow-up bd issue)
    let store = Arc::new(InMemoryEventStore::<DomainEvent>::new());
    let bus: Arc<FailingBus> = Arc::new(FailingBus);
    let runs_by_key: Arc<Mutex<HashMap<String, AggregateId>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let repos_by_key: Arc<Mutex<HashMap<String, AggregateId>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let deliveries_by_id: Arc<Mutex<HashMap<String, AggregateId>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let tracker: Arc<Mutex<HashMap<AggregateId, NonZeroU64>>> =
        Arc::new(Mutex::new(HashMap::new()));

    // Canonical Track 4.0 failure-injection seam: spawn the Merger
    // over the FailingBus via the test-only ctor. The merger arm
    // runs `publish_or_trace(&self.bus, ...)` after persistence
    // succeeds — exactly the absorb point under test.
    let (merger_tx, _merger_handle) = Merger::<FailingBus>::with_bus_for_test(
        Arc::clone(&store),
        Arc::clone(&bus),
        Arc::clone(&runs_by_key),
        Arc::clone(&repos_by_key),
        Arc::clone(&deliveries_by_id),
        Arc::clone(&tracker),
    );

    // Capture tracing events emitted while the helper runs. Use
    // `set_default` (returns a `DefaultGuard`) rather than
    // `with_default` so the dispatcher remains active across the
    // `.await` below; nesting a fresh runtime inside `#[tokio::test]`
    // is not permitted.
    let capture = CaptureLayer::default();
    let events_handle = Arc::clone(&capture.events);
    let subscriber = tracing_subscriber::registry().with(capture);
    let _guard = tracing::subscriber::set_default(subscriber);

    let cmd = StartSweep {
        org: "octocat".into(),
        repo_count: 1,
        batch_id: "m2a-prime-test".into(),
        timestamp: "2026-05-11T00:00:00Z".into(),
        snapshot_signature: "test-sig-m2a".into(),
    };
    let ctx = CorrelationContext::none();

    // Drive the Merger directly: this is the post-3b architectural
    // boundary at which `publish_or_trace` lives. The reply mirrors
    // what `RunService::start_sweep` would receive — start_sweep
    // returns `Ok(())` despite the bus failure (CHE-0024:R1).
    let (reply_tx, reply_rx) = oneshot::channel();
    merger_tx
        .send(MergerCommand::StartSweep {
            cmd,
            ctx,
            reply: reply_tx,
        })
        .await
        .expect("merger channel open");
    reply_rx
        .await
        .expect("merger reply delivered")
        .expect("start_sweep succeeds despite bus failure (CHE-0024:R1)");

    let captured = events_handle
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    // Filter to the EDA target we own.
    let eda_errors: Vec<&CapturedEvent> = captured
        .iter()
        .filter(|e| e.target == "gh_report.eda" && e.level == "ERROR")
        .collect();

    assert!(
        !eda_errors.is_empty(),
        "expected ≥1 ERROR-level event on gh_report.eda target; \
         got captured={captured:#?}",
    );

    // Per-envelope iteration: start_sweep produces exactly 1 envelope,
    // so we expect exactly 1 error emission for this single-envelope
    // batch.
    assert_eq!(
        eda_errors.len(),
        1,
        "one envelope ⇒ one error emission; per-envelope iteration. captured={eda_errors:#?}",
    );

    let evt = eda_errors[0];
    for required in [
        "event_id",
        "correlation_id",
        "causation_id",
        "aggregate_id",
        "error",
    ] {
        assert!(
            evt.fields.contains_key(required),
            "required structured field `{required}` missing from emitted event; \
             present fields={:?}",
            evt.fields.keys().collect::<Vec<_>>(),
        );
    }
}
