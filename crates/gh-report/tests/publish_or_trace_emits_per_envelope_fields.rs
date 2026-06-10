//! M2.a' — Proves the bus-publish failure absorb point inside the
//! lifted [`cherry_pit_merger::Merger`] emits a structured
//! `tracing::error!` per envelope with the five required fields
//! (`event_id`, `correlation_id`, `causation_id`, `aggregate_id`,
//! `error`) plus the existing `event` label.
//!
//! Cites: CHE-0024:R1 (publish failure non-fatal — events persisted),
//!        CHE-0024:R3 (consumers reconcile via replay from `EventStore::load`),
//!        COM-0019:R1 (structured emission at absorb point),
//!        COM-0019:R4 (`correlation_id` flows through observability boundary),
//!        COM-0019:R7 (`EventBus` retry-absorb telemetry — `error!` severity),
//!        CHE-0069:R7 (cherry-pit-merger's `publish_or_trace` shape).
//!
//! Post-Mission-H — the `publish_or_trace` absorb point lives inside
//! cherry-pit-merger's private `shared` module
//! (`crates/cherry-pit-merger/src/shared.rs`) and emits on the
//! `cherry_pit_merger` target. Pre-mission this lived in gh-report's
//! own `app::services::shared` and emitted on `gh_report.eda`. The
//! target string is the only observable change from the call-site
//! perspective; the field set is identical.
//!
//! Failure injection uses [`MergerHandles::with_bus_for_test`], the
//! gh-report-side test-only ctor that spawns three
//! per-aggregate [`cherry_pit_merger::Merger`] tasks against a
//! `FailingBus` test double.

use std::collections::HashMap;
use std::num::NonZeroU64;
use std::sync::{Arc, Mutex};

use cherry_pit_core::{AggregateId, BusError, CorrelationContext, EventBus, EventEnvelope};
use gh_report::app::state::EventStoreImpl;
use tempfile::TempDir;
use tracing::Subscriber;
use tracing::field::{Field, Visit};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::{Context, SubscriberExt};

use gh_report::app::services::MergerHandles;
use gh_report::app::services::repo_service::RepoService;
use gh_report::domain::aggregates::repo::RecordEvaluation;
use gh_report::domain::events::DomainEvent;

/// Fake `EventBus` that always returns `Err(BusError)` from `publish`.
/// Drives the lifted merger's `publish_or_trace` helper down its
/// error arm.
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
    let dir = TempDir::new().expect("tempdir");
    let store = Arc::new(EventStoreImpl::create_pgno(&dir.path().join("events.pgno")).unwrap());
    let bus: Arc<FailingBus> = Arc::new(FailingBus);
    let repos_by_key: Arc<Mutex<HashMap<String, AggregateId>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let tracker: Arc<Mutex<HashMap<AggregateId, NonZeroU64>>> =
        Arc::new(Mutex::new(HashMap::new()));

    let (handles, _joins) = MergerHandles::<FailingBus>::with_bus_for_test(
        Arc::clone(&store),
        Arc::clone(&bus),
        Arc::clone(&repos_by_key),
        Arc::clone(&tracker),
    );
    let svc = RepoService::with_handle(handles.repo);

    let capture = CaptureLayer::default();
    let events_handle = Arc::clone(&capture.events);
    let subscriber = tracing_subscriber::registry().with(capture);
    let _guard = tracing::subscriber::set_default(subscriber);

    let cmd = RecordEvaluation {
        domain_key: "id-m2a-prime-test".into(),
        repo_name: "m2a-prime-test".into(),
        success: true,
        source: "test".into(),
        duration_ms: 1,
        timestamp: "2026-05-11T00:00:00Z".into(),
        evidence: None,
    };
    let ctx = CorrelationContext::none();

    svc.record_evaluation("id-m2a-prime-test", cmd, &ctx)
        .await
        .expect("repo snapshot succeeds despite bus failure (CHE-0024:R1)");

    let captured = events_handle
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    let merger_errors: Vec<&CapturedEvent> = captured
        .iter()
        .filter(|e| e.target == "cherry_pit_merger" && e.level == "ERROR")
        .collect();

    assert!(
        !merger_errors.is_empty(),
        "expected ≥1 ERROR-level event on cherry_pit_merger target; \
         got captured={captured:#?}",
    );

    assert_eq!(
        merger_errors.len(),
        1,
        "one envelope ⇒ one error emission; per-envelope iteration. captured={merger_errors:#?}",
    );

    let evt = merger_errors[0];
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
