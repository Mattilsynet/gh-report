# CHE-0042. EventEnvelope Construction Invariants

Date: 2026-04-25
Last-reviewed: 2026-05-12
Tier: A
Status: Accepted

## Related

References: CHE-0002, CHE-0039, CHE-0016, COM-0025, GND-0005

## Context

`EventEnvelope<E>` wraps every domain event with metadata stamped by the `EventStore`. CHE-0016 establishes "callers never construct envelopes directly," but all seven fields are `pub` — any code can construct an envelope with wrong sequence, stale timestamp, or nil event_id. CHE-0002 says "encode invariants at the type level," yet `EventEnvelope` violates this. Three options were evaluated: private fields with validated constructor and accessors (enforced), `#[non_exhaustive]` on struct (partial), or accept with documentation (CHE-0002 violation persists).

## Decision

Option 1: Private fields + validated public constructor + accessor
methods.

R1 [5]: Construct EventEnvelope exclusively through
  EventEnvelope::new(), which validates non-nil event_id and
  returns Result<Self, EnvelopeError>
R2 [5]: Use NonZeroU64 for the EventEnvelope sequence field so
  zero sequences are rejected at the type level
R3 [5]: Access EventEnvelope fields through accessor methods
  (event_id(), aggregate_id(), sequence(), timestamp(), payload())
R4 [5]: Call EventEnvelope::validate_stream() after deserialization in
  EventStore::load implementations to catch corrupt stored data,
  aggregate_id mismatches, and sequence gaps

### Struct definition

```rust
#[derive(Debug, Clone, Serialize)]
#[serde(bound(serialize = "E: Serialize"))]
pub struct EventEnvelope<E: DomainEvent> {
    event_id: uuid::Uuid,
    aggregate_id: AggregateId,
    sequence: NonZeroU64,
    timestamp: jiff::Timestamp,
    correlation_id: Option<uuid::Uuid>,
    causation_id: Option<uuid::Uuid>,
    payload: E,
}
```

All fields private. `sequence` uses `NonZeroU64` to eliminate zero
sequences at the type level — serde rejects zero on
deserialization automatically. `Serialize` derived (serde can
access private fields within the defining module). `Deserialize`
handled separately (see below).

### Validated constructor

`EventEnvelope::new()` accepts all 7 fields as parameters and returns
`Result<Self, EnvelopeError>`. It validates that `event_id` is non-nil
(returns `EnvelopeError::NilEventId` otherwise), then assigns all
fields. Zero sequence is impossible at the type level via `NonZeroU64`.
See `cherry-pit-core/src/envelope.rs` for full implementation.

The constructor is `pub` (not `pub(crate)`) because `cherry-pit-gateway`
is a separate crate that must call it. The constructor is safe for
external use because it validates invariants — external callers
cannot create malformed envelopes through this path.

### Accessor methods

```rust
impl<E: DomainEvent> EventEnvelope<E> {
    pub fn event_id(&self) -> uuid::Uuid { self.event_id }
    pub fn aggregate_id(&self) -> AggregateId { self.aggregate_id }
    /// Returns the 1-based sequence; `NonZeroU64` enforces non-zero
    /// at the type level (CHE-0011 boundary-conversion doctrine).
    pub fn sequence(&self) -> NonZeroU64 { self.sequence }
    pub fn timestamp(&self) -> jiff::Timestamp { self.timestamp }
    pub fn correlation_id(&self) -> Option<uuid::Uuid> {
        self.correlation_id
    }
    pub fn causation_id(&self) -> Option<uuid::Uuid> {
        self.causation_id
    }
    pub fn payload(&self) -> &E { &self.payload }
}
```

### Error type

```rust
#[derive(Debug)]
pub enum EnvelopeError {
    /// event_id must not be the nil UUID.
    NilEventId,
}
```

Single variant — `ZeroSequence` is eliminated by using `NonZeroU64`
for the `sequence` field. Manual `Display` and `Error` impls,
consistent with CHE-0027.

### Serde deserialization

Rust's `#[derive(Deserialize)]` can access private fields within
the defining module. Deserialization reconstructs `EventEnvelope`
without calling `new()` — this bypasses constructor validation.

Mitigation: **post-deserialization stream validation in `EventStore::load`
implementations.** After deserializing envelopes from storage, the
store calls `EventEnvelope::validate_stream(aggregate_id, envelopes)`:

```rust
impl<E: DomainEvent> EventEnvelope<E> {
    pub fn validate_stream(
        aggregate_id: AggregateId,
        stream: &[Self],
    ) -> Result<(), EnvelopeError>;
}
```

This keeps the `Deserialize` derive simple (no custom impl with
complex generic bounds) while ensuring invalid data from corrupt
storage is caught at load time. Stream validation rejects nil event
IDs, envelopes whose aggregate_id differs from the requested stream,
and sequences that are not exactly contiguous from 1..=N.

## Consequences

- **CHE-0002 compliance restored.** External code cannot construct malformed envelopes. Validated constructor rejects nil event_id, `NonZeroU64` eliminates zero sequences, and post-deserialization validation catches corrupt stored data.
- **Breaking change.** 48 locations in `cherry-pit-gateway` change (field accesses → accessors, struct literals → `new()` calls). All mechanical.
- **Sequencing dependency on CHE-0039** — constructor accepts correlation/causation IDs from `CorrelationContext`.
- **Serde bypass is defense-in-depth.** `validate_stream()` in `load()` catches corrupt data; the invalid-state window is contained within the store.
- Future compile-fail test (CHE-0028) should verify struct literal construction fails.
