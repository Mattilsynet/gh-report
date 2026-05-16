//! `WebhookDelivery` aggregate — single GitHub webhook delivery
//! (CHE-0054:R3).
//!
//! Owns a single variant of [`DomainEvent`]: `WebhookReceived`.
//! Enforces the sole CHE-0054:R3 invariant: **exactly one
//! `WebhookReceived` event per instance** — write-once, terminal
//! after first event. Combined with delivery-id-keyed routing in
//! `AppState::deliveries_by_id` (CHE-0054:R5), this gives the
//! application-level "idempotency by delivery id" property that
//! currently lives in the in-memory `seen_deliveries` cache at
//! `webhook/mod.rs`.
//!
//! ## Degenerate-aggregate shape
//!
//! WebhookDelivery is a deliberately-degenerate aggregate per
//! CHE-0054:R3 / Consequences §2: single variant, single invariant,
//! no continuing lifecycle. The shape is preserved (rather than
//! reducing webhook ingest to an adapter-only port) so that future
//! enrichment (delivery-status events, retry metadata) is a
//! non-breaking R3 amendment.
//!
//! Per pre-mortem-#3 degeneracy check (mid-checkpoint instructions):
//! the existing `seen_deliveries` cache in `webhook/mod.rs:140` is a
//! real read-side consumer that an event-sourced WebhookDelivery
//! aggregate can replace post-WU-7. The invariant ("at most one
//! `WebhookReceived` per delivery_id") is therefore non-trivial.
//!
//! Per CHE-0009:R1–R2, [`WebhookDelivery::apply`] is total and
//! infallible. Per CHE-0008:R1, [`HandleCommand`] is pure.

use cherry_pit_core::{Aggregate, Command, HandleCommand};

use crate::domain::events::DomainEvent;

/// WebhookDelivery lifecycle phase derived from applied events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DeliveryPhase {
    /// No events applied yet — empty aggregate (CHE-0012:R1).
    #[default]
    Empty,
    /// `WebhookReceived` applied; terminal (CHE-0054:R3).
    Received,
}

/// The `WebhookDelivery` aggregate (CHE-0054:R3).
///
/// Per-instance state is intentionally minimal — the load-bearing
/// identity (`delivery_id`) lives in `AppState::deliveries_by_id` per
/// CHE-0054:R5. The aggregate captures only the action+repo
/// projection of the received event for replay equivalence.
#[derive(Debug, Clone, Default)]
pub struct WebhookDelivery {
    /// Current lifecycle phase.
    pub phase: DeliveryPhase,
    /// Mapped action from the received event (e.g. `"enqueue"`,
    /// `"remove"`, `"ignore"`), if any.
    pub action: Option<String>,
    /// Repo name from the received event, if applicable.
    pub repo: Option<String>,
}

impl Aggregate for WebhookDelivery {
    type Event = DomainEvent;

    fn apply(&mut self, event: &Self::Event) {
        match event {
            DomainEvent::WebhookReceived { action, repo, .. } => {
                self.phase = DeliveryPhase::Received;
                self.action = Some(action.clone());
                self.repo.clone_from(repo);
            }
            // Non-WebhookDelivery variants — defensively ignored.
            // Routing is the application boundary's responsibility
            // (CHE-0054:R5). `debug_assert!` (linus mid-review Info-1)
            // traps mis-routing in dev/test; release preserves
            // silent-ignore.
            DomainEvent::SweepStarted { .. }
            | DomainEvent::SweepProgress { .. }
            | DomainEvent::SweepCompleted { .. }
            | DomainEvent::SweepFailed { .. }
            | DomainEvent::EvidencePublished { .. }
            | DomainEvent::RepoEvaluated { .. }
            | DomainEvent::RepoRemoved { .. } => {
                debug_assert!(
                    false,
                    "WebhookDelivery::apply received non-WebhookDelivery variant: {event:?} (CHE-0054:R5 routing bug)"
                );
            }
        }
    }
}

/// Errors rejecting commands against `WebhookDelivery` invariants
/// (CHE-0054:R3).
///
/// `#[non_exhaustive]` per linus L1 — B7'b/c may add variants for
/// delivery-status enrichment.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum WebhookError {
    /// At-most-one-event invariant violated: this delivery has already
    /// been recorded (CHE-0054:R3). The delivery-id-keyed
    /// `AppState::deliveries_by_id` should normally route a duplicate
    /// command to a fresh aggregate via the resolved AggregateId, so
    /// reaching this error is itself a routing-cache miss (legitimate
    /// idempotent retry path).
    #[error("WebhookDelivery already received (terminal)")]
    AlreadyReceived,
}

// --- Commands (CHE-0054:R4 use cases) ---------------------------------

/// Record a single received GitHub webhook delivery.
///
/// `delivery_id` is the load-bearing identity carried at the
/// application boundary (`AppState::deliveries_by_id` per CHE-0054:R5);
/// it is included on the command for completeness and for use by the
/// routing layer, but is not currently materialised on
/// [`DomainEvent::WebhookReceived`] (open ζ-2 — adding the field
/// would break msgpack envelope replay; deferred).
#[derive(Debug, Clone)]
pub struct RecordDelivery {
    /// GitHub `X-GitHub-Delivery` header value — the load-bearing
    /// idempotency key for this delivery (CHE-0054:R3).
    pub delivery_id: String,
    /// Mapped action — one of `"enqueue"`, `"remove"`, `"ignore"`.
    /// Opaque to the aggregate; produced by the webhook adapter from
    /// the GitHub event type + payload.
    pub action: String,
    /// Repository name, if the delivery references a repo.
    pub repo: Option<String>,
    /// ISO 8601 UTC timestamp.
    pub timestamp: String,
}
impl Command for RecordDelivery {}

// --- HandleCommand impls (CHE-0008:R1 pure) ---------------------------

impl HandleCommand<RecordDelivery> for WebhookDelivery {
    type Error = WebhookError;

    fn handle(&self, cmd: RecordDelivery) -> Result<Vec<DomainEvent>, Self::Error> {
        if self.phase == DeliveryPhase::Received {
            return Err(WebhookError::AlreadyReceived);
        }
        Ok(vec![DomainEvent::WebhookReceived {
            action: cmd.action,
            repo: cmd.repo,
            timestamp: cmd.timestamp,
        }])
    }
}

// --- Tests (CHE-0008:R3 pure-handle unit tests) -----------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn ts() -> String {
        "2026-05-10T12:00:00Z".to_string()
    }

    // --- apply (CHE-0009 infallible) ---

    #[test]
    fn default_delivery_is_empty() {
        let d = WebhookDelivery::default();
        assert_eq!(d.phase, DeliveryPhase::Empty);
        assert!(d.action.is_none());
        assert!(d.repo.is_none());
    }

    #[test]
    fn apply_webhook_received_sets_received_phase() {
        let mut d = WebhookDelivery::default();
        d.apply(&DomainEvent::WebhookReceived {
            action: "enqueue".into(),
            repo: Some("my-repo".into()),
            timestamp: ts(),
        });
        assert_eq!(d.phase, DeliveryPhase::Received);
        assert_eq!(d.action.as_deref(), Some("enqueue"));
        assert_eq!(d.repo.as_deref(), Some("my-repo"));
    }

    #[test]
    fn apply_webhook_received_no_repo() {
        let mut d = WebhookDelivery::default();
        d.apply(&DomainEvent::WebhookReceived {
            action: "ignore".into(),
            repo: None,
            timestamp: ts(),
        });
        assert_eq!(d.phase, DeliveryPhase::Received);
        assert_eq!(d.action.as_deref(), Some("ignore"));
        assert!(d.repo.is_none());
    }

    #[test]
    #[should_panic(expected = "CHE-0054:R5 routing bug")]
    fn apply_panics_in_debug_on_non_delivery_variant() {
        // Per linus mid-review Info-1: dev/test traps mis-routing,
        // release silently ignores (debug_assert no-ops). Only the
        // dev-time trap is observable in tests.
        let mut d = WebhookDelivery::default();
        d.apply(&DomainEvent::SweepStarted {
            org: "o".into(),
            repo_count: 1,
            batch_id: "b".into(),
            timestamp: ts(),
        });
    }

    // --- handle (CHE-0008 pure) ---

    #[test]
    fn record_delivery_from_empty_emits_event() {
        let d = WebhookDelivery::default();
        let events = d
            .handle(RecordDelivery {
                delivery_id: "abc-123".into(),
                action: "enqueue".into(),
                repo: Some("my-repo".into()),
                timestamp: ts(),
            })
            .unwrap();
        assert_eq!(events.len(), 1);
        match &events[0] {
            DomainEvent::WebhookReceived { action, repo, .. } => {
                assert_eq!(action, "enqueue");
                assert_eq!(repo.as_deref(), Some("my-repo"));
            }
            other => panic!("expected WebhookReceived, got {other:?}"),
        }
    }

    #[test]
    fn record_delivery_after_received_rejects() {
        // CHE-0054:R3 — at most one WebhookReceived per instance.
        // The application-layer routing-cache miss path: a duplicate
        // command for the same delivery_id reaches a freshly-loaded
        // aggregate that already replayed the original event.
        let mut d = WebhookDelivery::default();
        d.apply(&DomainEvent::WebhookReceived {
            action: "enqueue".into(),
            repo: Some("my-repo".into()),
            timestamp: ts(),
        });
        let err = d
            .handle(RecordDelivery {
                delivery_id: "abc-123".into(),
                action: "enqueue".into(),
                repo: Some("my-repo".into()),
                timestamp: ts(),
            })
            .unwrap_err();
        assert_eq!(err, WebhookError::AlreadyReceived);
    }

    #[test]
    fn record_delivery_ignore_action_emits_event() {
        // Non-actionable webhooks (action="ignore") still produce an
        // audit-trail event per CHE-0024:R1 persist-then-publish for
        // any side-effecting input.
        let d = WebhookDelivery::default();
        let events = d
            .handle(RecordDelivery {
                delivery_id: "xyz-789".into(),
                action: "ignore".into(),
                repo: None,
                timestamp: ts(),
            })
            .unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], DomainEvent::WebhookReceived { .. }));
    }
}
