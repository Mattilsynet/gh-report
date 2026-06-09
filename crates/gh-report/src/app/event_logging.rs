//! Logging subscriber registration for the domain event bus.
//!
//! Lives in the `app/` layer rather than `domain/` because the
//! registration couples a `tracing::info!` sink (observability) to an
//! `InProcessEventBus<DomainEvent>` (infrastructure handle). Domain code
//! must not depend on either; orchestration of cross-cutting concerns is
//! an app-layer responsibility (COM-0012:R3).

use cherry_pit_agent::InProcessEventBus;
use cherry_pit_core::EventEnvelope;
use tracing::info;

use crate::domain::events::DomainEvent;

/// Register a synchronous handler that logs every published domain event
/// via `tracing::info!`.
///
/// Proves fan-out: this handler runs alongside any other handlers
/// (metrics, persistence, etc.) registered on the same
/// `InProcessEventBus` — each receives every envelope per CHE-0024:§7
/// (synchronous in-process delivery).
///
/// # Lifecycle
///
/// There is no task to manage: the handler is a closure invoked
/// synchronously inside `bus.publish(...)` (CHE-0051:R2). Registration
/// is permanent for the lifetime of the bus; there is no unsubscribe.
pub fn register_logging_subscriber(bus: &InProcessEventBus<DomainEvent>) {
    bus.register(|envelope: &EventEnvelope<DomainEvent>| {
        let event = envelope.payload();
        info!(
            domain_event = %event,
            event_type = event.event_type(),
            "domain event"
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now_str() -> String {
        jiff::Timestamp::now().to_string()
    }

    #[tokio::test]
    async fn logging_subscriber_receives_events() {
        use cherry_pit_core::{AggregateId, EventBus, EventEnvelope};
        use std::num::NonZeroU64;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicU32, Ordering};

        fn envelope(payload: DomainEvent, sequence: u64) -> EventEnvelope<DomainEvent> {
            EventEnvelope::new(
                uuid::Uuid::now_v7(),
                AggregateId::new(NonZeroU64::new(1).expect("non-zero")),
                NonZeroU64::new(sequence).expect("non-zero"),
                jiff::Timestamp::now(),
                None,
                None,
                payload,
            )
            .expect("valid envelope")
        }

        let bus: InProcessEventBus<DomainEvent> = InProcessEventBus::new();

        register_logging_subscriber(&bus);
        let counter = Arc::new(AtomicU32::new(0));
        {
            let counter = Arc::clone(&counter);
            bus.register(move |_env: &EventEnvelope<DomainEvent>| {
                counter.fetch_add(1, Ordering::Relaxed);
            });
        }
        assert_eq!(
            bus.handler_count(),
            2,
            "logging + counter handler should both be registered"
        );

        bus.publish(&[
            envelope(
                DomainEvent::SweepStarted {
                    org: "test-org".into(),
                    repo_count: 3,
                    batch_id: "test-batch".into(),
                    timestamp: now_str(),
                    snapshot_signature: None,
                },
                1,
            ),
            envelope(
                DomainEvent::RepoEvaluated {
                    domain_key: "k1".into(),
                    repo_name: "repo-1".into(),
                    success: true,
                    source: "test".into(),
                    duration_ms: 50,
                    timestamp: now_str(),
                    evidence: None,
                },
                2,
            ),
        ])
        .await
        .expect("publish should not fail");

        assert_eq!(
            counter.load(Ordering::Relaxed),
            2,
            "sibling counter should have observed both events synchronously",
        );
    }

    #[tokio::test]
    async fn logging_subscriber_fan_out_both_receive() {
        use cherry_pit_core::{AggregateId, EventBus, EventEnvelope};
        use std::num::NonZeroU64;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicU32, Ordering};

        fn envelope(payload: DomainEvent, sequence: u64) -> EventEnvelope<DomainEvent> {
            EventEnvelope::new(
                uuid::Uuid::now_v7(),
                AggregateId::new(NonZeroU64::new(1).expect("non-zero")),
                NonZeroU64::new(sequence).expect("non-zero"),
                jiff::Timestamp::now(),
                None,
                None,
                payload,
            )
            .expect("valid envelope")
        }

        let bus: InProcessEventBus<DomainEvent> = InProcessEventBus::new();

        let counter = Arc::new(AtomicU32::new(0));
        {
            let counter = Arc::clone(&counter);
            bus.register(move |_env: &EventEnvelope<DomainEvent>| {
                counter.fetch_add(1, Ordering::Relaxed);
            });
        }
        register_logging_subscriber(&bus);
        assert_eq!(bus.handler_count(), 2, "two handlers registered");

        let envelopes: Vec<EventEnvelope<DomainEvent>> = (0u64..3)
            .map(|i| {
                envelope(
                    DomainEvent::RepoEvaluated {
                        domain_key: format!("k{i}"),
                        repo_name: format!("repo-{i}"),
                        success: true,
                        source: "test".into(),
                        duration_ms: 10,
                        timestamp: now_str(),
                        evidence: None,
                    },
                    i + 1,
                )
            })
            .collect();
        bus.publish(&envelopes).await.expect("publish ok");

        assert_eq!(
            counter.load(Ordering::Relaxed),
            3,
            "counter should see all 3 events (fan-out)",
        );
    }
}
