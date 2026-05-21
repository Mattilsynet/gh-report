use serde::Serialize;
use serde::de::DeserializeOwned;
use std::num::NonZeroU64;

use crate::aggregate_id::AggregateId;
use crate::error::EnvelopeError;

/// Marker trait for domain events.
///
/// Events are immutable facts — something that happened. They are the
/// source of truth in an event-sourced system. Every event must be
/// cloneable (for fan-out to multiple consumers) and serde-serializable
/// for substrate-side persistence + transport.
/// (CHE-0010 R1–R3 [amended; see ADR-debt issue]: supertrait bounds
/// `Clone` + `Send` + `Sync` + `'static` + `serde::Serialize` +
/// `serde::de::DeserializeOwned`. Pardosa-encoding has been removed
/// from the workspace; the wire format is now msgpack via `rmp-serde`.
/// CHE-0022 R1–R5: event enum evolution rules — no `#[non_exhaustive]`,
/// immutable `event_type()` strings, new fields as `Option<T>`;
/// CHE-0045 R1–R2: domain events format-agnostic, serde chosen by infra.
/// ADR cleanup deferred per user mission scope (pardosa-deletion-1779100000):
/// CHE-0064:R2 supersession ADR not yet written.)
///
/// # Examples
///
/// ```
/// use cherry_pit_core::DomainEvent;
/// use serde::{Serialize, Deserialize};
///
/// #[derive(Debug, Clone, Serialize, Deserialize)]
/// enum OrderEvent {
///     Placed { item: String },
/// }
///
/// impl DomainEvent for OrderEvent {
///     fn event_type(&self) -> &'static str {
///         match self {
///             OrderEvent::Placed { .. } => "order.placed",
///         }
///     }
/// }
/// ```
pub trait DomainEvent: Clone + Send + Sync + 'static + Serialize + DeserializeOwned {
    /// A stable string identifier for this event type.
    ///
    /// Used for routing, schema registry, and deserialization dispatch.
    /// Must not change once events of this type exist in a log.
    fn event_type(&self) -> &'static str;
}

/// Infrastructure wrapper around a domain event.
/// (CHE-0016 R1–R3: store creates envelopes, correlation/causation;
/// CHE-0033 R1–R3: UUID v7 `event_id`, sequence ordering;
/// CHE-0034 R1: `jiff::Timestamp` for temporal values;
/// CHE-0042 R1–R4: validated construction, `NonZeroU64` sequence,
/// private fields, `validate_stream` after deserialization.)
///
/// Provided by cherry-pit-core, not implemented by the agent. This is what
/// gets persisted and transported. The domain event is the payload;
/// the envelope adds the metadata needed for ordering, routing, and
/// idempotency.
///
/// Envelopes are created by the [`EventStore`](crate::EventStore)
/// during `create` and `append` — callers pass raw domain events,
/// the store stamps on the metadata.
///
/// # Construction
///
/// Fields are private — use [`EventEnvelope::new()`] to construct.
/// The constructor validates invariants (non-nil `event_id`); the
/// `sequence` field uses [`NonZeroU64`] to eliminate zero sequences
/// at the type level.
///
/// # Correlation and causation
///
/// `correlation_id` groups related events across aggregates and
/// bounded contexts into a single logical operation. All events
/// produced by a command (and any downstream commands triggered by
/// policies) share the same `correlation_id`.
///
/// `causation_id` identifies the specific event that caused this
/// event to be produced. For events produced directly by a command,
/// `causation_id` is `None`. For events produced by a policy
/// reacting to a prior event, `causation_id` points to that prior
/// event's `event_id`.
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
#[serde(bound(serialize = "E: Serialize", deserialize = "E: DeserializeOwned"))]
pub struct EventEnvelope<E: DomainEvent> {
    /// Unique identifier for this event instance (UUID v7, time-ordered).
    event_id: uuid::Uuid,

    /// The aggregate instance this event belongs to (stream partition key).
    aggregate_id: AggregateId,

    /// Monotonically increasing sequence within the aggregate's stream.
    /// Uses `NonZeroU64` — sequences start at 1, never 0.
    sequence: NonZeroU64,

    /// When this event was created (UTC instant).
    timestamp: jiff::Timestamp,

    /// Correlation ID grouping related events across aggregates into
    /// a single logical operation. Propagated through policies and
    /// sagas.
    #[serde(default)]
    correlation_id: Option<uuid::Uuid>,

    /// The `event_id` of the event that caused this event to be
    /// produced (via a policy or saga). `None` for events produced
    /// directly by a user-initiated command.
    #[serde(default)]
    causation_id: Option<uuid::Uuid>,

    /// The domain event payload.
    payload: E,
}

impl<E: DomainEvent> EventEnvelope<E> {
    /// Construct a new envelope with validated invariants.
    ///
    /// # Errors
    ///
    /// Returns [`EnvelopeError::NilEventId`] if `event_id` is
    /// [`Uuid::nil()`](uuid::Uuid::nil).
    /// (CHE-0042 R1: validated construction.)
    ///
    /// # Examples
    ///
    /// ```
    /// use std::num::NonZeroU64;
    /// use cherry_pit_core::{AggregateId, DomainEvent, EventEnvelope, EnvelopeError};
    /// use serde::{Serialize, Deserialize};
    ///
    /// #[derive(Debug, Clone, Serialize, Deserialize)]
    /// enum Ev { Created }
    /// impl DomainEvent for Ev {
    ///     fn event_type(&self) -> &'static str { "ev.created" }
    /// }
    ///
    /// let id = AggregateId::new(NonZeroU64::new(1).unwrap());
    /// let seq = NonZeroU64::new(1).unwrap();
    ///
    /// // Valid construction succeeds.
    /// let ok = EventEnvelope::new(
    ///     uuid::Uuid::now_v7(), id, seq,
    ///     jiff::Timestamp::now(), None, None, Ev::Created,
    /// );
    /// assert!(ok.is_ok());
    ///
    /// // Nil event_id is rejected — CHE-0042.
    /// let err = EventEnvelope::new(
    ///     uuid::Uuid::nil(), id, seq,
    ///     jiff::Timestamp::now(), None, None, Ev::Created,
    /// );
    /// assert!(err.is_err());
    /// ```
    pub fn new(
        event_id: uuid::Uuid,
        aggregate_id: AggregateId,
        sequence: NonZeroU64,
        timestamp: jiff::Timestamp,
        correlation_id: Option<uuid::Uuid>,
        causation_id: Option<uuid::Uuid>,
        payload: E,
    ) -> Result<Self, EnvelopeError> {
        if event_id.is_nil() {
            return Err(EnvelopeError::NilEventId);
        }
        Ok(Self {
            event_id,
            aggregate_id,
            sequence,
            timestamp,
            correlation_id,
            causation_id,
            payload,
        })
    }

    /// Validate a deserialized envelope.
    ///
    /// Defense-in-depth: call after deserializing from storage to
    /// catch corrupted data early. Checks the same invariants as
    /// [`new()`](Self::new).
    ///
    /// # Errors
    ///
    /// Returns [`EnvelopeError::NilEventId`] if `event_id` is nil.
    pub fn validate(&self) -> Result<(), EnvelopeError> {
        if self.event_id.is_nil() {
            return Err(EnvelopeError::NilEventId);
        }
        // NonZeroU64 guarantees sequence > 0 — no runtime check needed.
        Ok(())
    }

    /// Validate a full aggregate stream after deserialization.
    ///
    /// This enforces the replay contract for one stream: every envelope
    /// belongs to the requested aggregate, and sequences are exactly
    /// contiguous from 1 through `stream.len()`. The check detects gaps,
    /// duplicates, out-of-order events, and cross-stream corruption before
    /// state is rebuilt from persisted facts.
    ///
    /// # Errors
    ///
    /// Returns [`EnvelopeError::NilEventId`] for malformed event identity,
    /// [`EnvelopeError::AggregateIdMismatch`] for cross-stream data, or
    /// [`EnvelopeError::SequenceGap`] for non-contiguous sequence numbers.
    pub fn validate_stream(
        aggregate_id: AggregateId,
        stream: &[Self],
    ) -> Result<(), EnvelopeError> {
        for (index, envelope) in stream.iter().enumerate() {
            envelope.validate()?;

            if envelope.aggregate_id != aggregate_id {
                return Err(EnvelopeError::AggregateIdMismatch {
                    expected: aggregate_id,
                    actual: envelope.aggregate_id,
                });
            }

            let expected_sequence = u64::try_from(index)
                .ok()
                .and_then(|i| i.checked_add(1))
                .unwrap_or(u64::MAX);
            if envelope.sequence().get() != expected_sequence {
                return Err(EnvelopeError::SequenceGap {
                    expected_sequence,
                    actual_sequence: envelope.sequence(),
                });
            }
        }

        Ok(())
    }

    /// Returns the envelope's stable event identity.
    ///
    /// Per CHE-0033, the identifier is a UUID v7 whose embedded
    /// Unix-millisecond timestamp yields a monotonic-ish lexicographic
    /// ordering across emitters, enabling cheap time-based shard keys and
    /// index locality without a coordinator. This is the identity the
    /// `EventStore` uses to detect duplicate deliveries per CHE-0024's
    /// at-least-once delivery + idempotency contract: replaying the same
    /// envelope MUST be a no-op, keyed by `event_id`.
    #[must_use]
    pub fn event_id(&self) -> uuid::Uuid {
        self.event_id
    }

    /// Returns the aggregate this envelope is bound to.
    ///
    /// An `EventEnvelope` is **immutably bound** to exactly one aggregate
    /// at construction (CHE-0042:R1); the binding cannot be rewritten.
    /// Stream replay therefore requires that every envelope in the slice
    /// share this `aggregate_id` — `validate_stream` (CHE-0042:R4) rejects
    /// any mixed-aggregate slice before the projection sees it.
    #[must_use]
    pub fn aggregate_id(&self) -> AggregateId {
        self.aggregate_id
    }

    /// The 1-based sequence number within the aggregate's stream.
    ///
    /// Exposes the internal `NonZeroU64` invariant directly. Callers
    /// entering from raw `u64` (e.g. checkpoint counters that may be 0)
    /// must validate at the integer-entry boundary; this accessor never
    /// returns 0.
    #[must_use]
    pub fn sequence(&self) -> NonZeroU64 {
        self.sequence
    }

    /// Returns the wall-clock time the event was emitted by its producer.
    ///
    /// Per COM-0025:R5 timestamps are **observational metadata** — useful
    /// for audit, display, and coarse retention windows, but NOT
    /// authoritative for ordering. Per-stream ordering is established by
    /// `sequence()`; no global cross-stream order is implied by comparing
    /// timestamps across aggregates, since clocks may drift, skew, or run
    /// backwards across producers.
    #[must_use]
    pub fn timestamp(&self) -> jiff::Timestamp {
        self.timestamp
    }

    /// Returns the correlation identifier propagated with this event, if any.
    ///
    /// Per CHE-0039, all events emitted while handling the same inbound
    /// request — directly or via downstream sagas — share a single
    /// `correlation_id`. The value is allocated once at the gateway edge
    /// and propagated through `CorrelationContext`, never generated
    /// per-event, so equality of `correlation_id` uniquely groups a
    /// request's full causal fan-out for tracing and idempotency.
    #[must_use]
    pub fn correlation_id(&self) -> Option<uuid::Uuid> {
        self.correlation_id
    }

    /// Returns the `event_id` of the message that directly caused this event, if any.
    ///
    /// Per CHE-0039 causation is a **parent pointer**: the `event_id` (or
    /// command id) of the single immediately-preceding message in the
    /// causal chain. Distinct from `correlation_id`, which is
    /// request-scoped and shared by every message in the fan-out;
    /// `causation_id` walks one edge up the DAG and is unique per parent.
    #[must_use]
    pub fn causation_id(&self) -> Option<uuid::Uuid> {
        self.causation_id
    }

    /// Borrows the domain event payload carried by this envelope.
    ///
    /// The borrowed reference is intentional: per CHE-0042 envelopes are
    /// immutable post-construction, so the payload never needs to be
    /// cloned to be observed. Mutation of a stored event is impossible by
    /// design — combined with CHE-0009's infallible `apply`, this
    /// guarantees that replay over a fixed stream is deterministic and
    /// side-effect-free.
    #[must_use]
    pub fn payload(&self) -> &E {
        &self.payload
    }
}

/// Canonical serialization of an envelope is via serde + msgpack
/// (`rmp-serde`). The struct's serde derive defines the wire format.
/// Field order and per-field encoding follow serde's derived layout —
/// treat the resulting bytes as a wire format.
///
/// (ADR cleanup deferred per user mission scope: pardosa-encoding
/// `Encode`/`Decode` impls have been removed alongside the pardosa
/// crates; the prior CHE-0064 hash-chain pre-image is no longer
/// applicable in this workspace.)
#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    enum TestEvent {
        Happened { value: String },
    }

    impl DomainEvent for TestEvent {
        fn event_type(&self) -> &'static str {
            "test.happened"
        }
    }

    proptest! {
        #[test]
        fn envelope_msgpack_roundtrip(
            seq in 1..=u64::MAX,
            value in "[a-zA-Z0-9]{0,50}",
        ) {
            let id = AggregateId::new(NonZeroU64::new(1).unwrap());
            let sequence = NonZeroU64::new(seq).unwrap();
            let envelope = EventEnvelope::new(
                uuid::Uuid::now_v7(),
                id,
                sequence,
                jiff::Timestamp::now(),
                None,
                None,
                TestEvent::Happened { value: value.clone() },
            ).unwrap();

            let bytes = rmp_serde::encode::to_vec_named(&envelope).unwrap();
            let back: EventEnvelope<TestEvent> = rmp_serde::from_slice(&bytes).unwrap();

            prop_assert_eq!(back.event_id(), envelope.event_id());
            prop_assert_eq!(back.aggregate_id(), envelope.aggregate_id());
            prop_assert_eq!(back.sequence(), envelope.sequence());
            prop_assert_eq!(back.payload(), envelope.payload());
        }
    }

    #[test]
    fn envelope_msgpack_roundtrip_with_correlation_and_causation() {
        // Exercise the Some(uuid) branches of correlation_id/causation_id,
        // and sequence > 1 with a multi-byte payload.
        let id = AggregateId::new(NonZeroU64::new(42).unwrap());
        let envelope = EventEnvelope::new(
            uuid::Uuid::now_v7(),
            id,
            NonZeroU64::new(7).unwrap(),
            jiff::Timestamp::from_second(1_700_000_000).unwrap(),
            Some(uuid::Uuid::now_v7()),
            Some(uuid::Uuid::now_v7()),
            TestEvent::Happened {
                value: "multi-byte payload ✓".into(),
            },
        )
        .unwrap();

        let bytes = rmp_serde::encode::to_vec_named(&envelope).unwrap();
        let back: EventEnvelope<TestEvent> = rmp_serde::from_slice(&bytes).unwrap();

        assert_eq!(back.event_id(), envelope.event_id());
        assert_eq!(back.aggregate_id(), envelope.aggregate_id());
        assert_eq!(back.sequence(), envelope.sequence());
        assert_eq!(back.timestamp(), envelope.timestamp());
        assert_eq!(back.correlation_id(), envelope.correlation_id());
        assert_eq!(back.causation_id(), envelope.causation_id());
        assert_eq!(back.payload(), envelope.payload());
    }

    #[test]
    fn new_rejects_nil_event_id() {
        let result = EventEnvelope::new(
            uuid::Uuid::nil(),
            AggregateId::new(NonZeroU64::new(1).unwrap()),
            NonZeroU64::new(1).unwrap(),
            jiff::Timestamp::now(),
            None,
            None,
            TestEvent::Happened { value: "x".into() },
        );
        assert!(matches!(result, Err(EnvelopeError::NilEventId)));
    }

    #[test]
    fn new_accepts_valid_envelope() {
        let result = EventEnvelope::new(
            uuid::Uuid::now_v7(),
            AggregateId::new(NonZeroU64::new(1).unwrap()),
            NonZeroU64::new(1).unwrap(),
            jiff::Timestamp::now(),
            Some(uuid::Uuid::now_v7()),
            Some(uuid::Uuid::now_v7()),
            TestEvent::Happened { value: "ok".into() },
        );
        assert!(result.is_ok());
    }

    #[test]
    fn validate_catches_nil_event_id() {
        // Construct via serde bypass (deserializing crafted msgpack).
        let nil_id = uuid::Uuid::nil();
        let id = AggregateId::new(NonZeroU64::new(1).unwrap());

        // Manually build a valid envelope then serialize it with a
        // nil event_id by crafting the struct directly (we're in the
        // same module, so we can access private fields).
        let bad_envelope = EventEnvelope {
            event_id: nil_id,
            aggregate_id: id,
            sequence: NonZeroU64::new(1).unwrap(),
            timestamp: jiff::Timestamp::now(),
            correlation_id: None,
            causation_id: None,
            payload: TestEvent::Happened {
                value: "bad".into(),
            },
        };

        assert!(matches!(
            bad_envelope.validate(),
            Err(EnvelopeError::NilEventId)
        ));
    }

    #[test]
    fn validate_passes_for_valid_envelope() {
        let envelope = EventEnvelope::new(
            uuid::Uuid::now_v7(),
            AggregateId::new(NonZeroU64::new(1).unwrap()),
            NonZeroU64::new(5).unwrap(),
            jiff::Timestamp::now(),
            None,
            None,
            TestEvent::Happened { value: "ok".into() },
        )
        .unwrap();

        assert!(envelope.validate().is_ok());
    }

    #[test]
    fn validate_stream_accepts_contiguous_stream() {
        let id = AggregateId::new(NonZeroU64::new(1).unwrap());
        let stream = vec![
            EventEnvelope::new(
                uuid::Uuid::now_v7(),
                id,
                NonZeroU64::new(1).unwrap(),
                jiff::Timestamp::now(),
                None,
                None,
                TestEvent::Happened { value: "a".into() },
            )
            .unwrap(),
            EventEnvelope::new(
                uuid::Uuid::now_v7(),
                id,
                NonZeroU64::new(2).unwrap(),
                jiff::Timestamp::now(),
                None,
                None,
                TestEvent::Happened { value: "b".into() },
            )
            .unwrap(),
        ];

        assert!(EventEnvelope::validate_stream(id, &stream).is_ok());
    }

    #[test]
    fn validate_stream_rejects_sequence_gap() {
        let id = AggregateId::new(NonZeroU64::new(1).unwrap());
        let stream = vec![
            EventEnvelope::new(
                uuid::Uuid::now_v7(),
                id,
                NonZeroU64::new(1).unwrap(),
                jiff::Timestamp::now(),
                None,
                None,
                TestEvent::Happened { value: "a".into() },
            )
            .unwrap(),
            EventEnvelope::new(
                uuid::Uuid::now_v7(),
                id,
                NonZeroU64::new(3).unwrap(),
                jiff::Timestamp::now(),
                None,
                None,
                TestEvent::Happened { value: "b".into() },
            )
            .unwrap(),
        ];

        assert!(matches!(
            EventEnvelope::validate_stream(id, &stream),
            Err(EnvelopeError::SequenceGap {
                expected_sequence: 2,
                actual_sequence,
            }) if actual_sequence.get() == 3
        ));
    }

    #[test]
    fn validate_stream_rejects_aggregate_mismatch() {
        let id = AggregateId::new(NonZeroU64::new(1).unwrap());
        let other_id = AggregateId::new(NonZeroU64::new(2).unwrap());
        let stream = vec![
            EventEnvelope::new(
                uuid::Uuid::now_v7(),
                other_id,
                NonZeroU64::new(1).unwrap(),
                jiff::Timestamp::now(),
                None,
                None,
                TestEvent::Happened { value: "a".into() },
            )
            .unwrap(),
        ];

        assert!(matches!(
            EventEnvelope::validate_stream(id, &stream),
            Err(EnvelopeError::AggregateIdMismatch { expected, actual })
                if expected == id && actual == other_id
        ));
    }

    // ── golden-file serde regression ───────────────────────────────

    /// Build a deterministic envelope with fixed values for golden-file
    /// comparison. Every field uses a hard-coded constant so the
    /// serialized bytes are reproducible across runs and platforms.
    fn golden_envelope() -> EventEnvelope<TestEvent> {
        let event_id = uuid::Uuid::from_bytes([
            0x01, 0x93, 0xa3, 0xe8, 0x80, 0x00, 0x7c, 0xde, 0x8f, 0x01, 0x23, 0x45, 0x67, 0x89,
            0xab, 0xcd,
        ]);
        let correlation_id = uuid::Uuid::from_bytes([
            0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x71, 0x22, 0x83, 0x44, 0x55, 0x66, 0x77, 0x88,
            0x99, 0x00,
        ]);
        let causation_id = uuid::Uuid::from_bytes([
            0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x89, 0xaa, 0xbb, 0xcc, 0xdd, 0xee,
            0xff, 0x00,
        ]);
        let aggregate_id = AggregateId::new(NonZeroU64::new(42).unwrap());
        let sequence = NonZeroU64::new(7).unwrap();
        let timestamp = jiff::Timestamp::from_second(1_700_000_000).unwrap();

        EventEnvelope {
            event_id,
            aggregate_id,
            sequence,
            timestamp,
            correlation_id: Some(correlation_id),
            causation_id: Some(causation_id),
            payload: TestEvent::Happened {
                value: "golden".into(),
            },
        }
    }

    /// Path to the golden-file fixture, relative to the crate root.
    fn golden_file_path() -> std::path::PathBuf {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        manifest.join("tests/fixtures/envelope_golden.msgpack")
    }

    #[test]
    fn envelope_serde_golden_file_roundtrip() {
        let envelope = golden_envelope();
        let serialized = rmp_serde::encode::to_vec_named(&envelope).unwrap();

        let path = golden_file_path();
        if !path.exists() {
            // First run — generate the fixture.
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, &serialized).unwrap();
            eprintln!(
                "Golden file written to {}. Commit this file.",
                path.display()
            );
        }

        let expected = std::fs::read(&path).unwrap();
        assert_eq!(
            serialized,
            expected,
            "Serialized envelope does not match golden file at {}. \
             If the change is intentional (schema evolution), update the \
             fixture and document in an ADR.",
            path.display()
        );

        // Deserialize the golden file and verify field values.
        let deserialized: EventEnvelope<TestEvent> = rmp_serde::from_slice(&expected).unwrap();
        deserialized.validate().unwrap();

        assert_eq!(deserialized.event_id(), envelope.event_id());
        assert_eq!(deserialized.aggregate_id(), envelope.aggregate_id());
        assert_eq!(deserialized.sequence(), envelope.sequence());
        assert_eq!(deserialized.timestamp(), envelope.timestamp());
        assert_eq!(deserialized.correlation_id(), envelope.correlation_id());
        assert_eq!(deserialized.causation_id(), envelope.causation_id());
        assert_eq!(deserialized.payload(), envelope.payload());
    }
}
