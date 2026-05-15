# CHE-0034. jiff::Timestamp as Temporal Foundation

Date: 2026-04-25
Last-reviewed: 2026-04-28
Tier: D
Status: Accepted

## Related

References: CHE-0016

## Context

`EventEnvelope` stores a `timestamp` field appearing in every persisted event, projection, and policy reaction. `chrono` has surprising fallible arithmetic at DST transitions. `time` has a less mature timezone ecosystem. `std::time::SystemTime` lacks serde and formatting. `jiff::Timestamp` (by BurntSushi) provides UTC-instant semantics, lossless RFC 9557/RFC 3339 serde roundtrips, DST-safe arithmetic, and built-in IANA timezone support without a separate crate.

## Decision

All temporal values use `jiff::Timestamp`, providing UTC-instant semantics
with lossless RFC 9557/RFC 3339 serde roundtrips, DST-safe arithmetic, and
built-in IANA timezone support without a separate crate.

R1 [10]: Use jiff::Timestamp for all temporal values in the framework
R2 [10]: Call Timestamp::now() once per batch so all events in an
  atomic batch share the same timestamp
R3 [10]: Treat jiff::Timestamp as observational metadata; use
  EventEnvelope::sequence for per-stream order and explicit
  correlation identifiers for cross-stream causality

```rust
// EventEnvelope field
pub timestamp: jiff::Timestamp,

// Generated in MsgpackFileStore::build_envelopes
let timestamp = jiff::Timestamp::now();
```

Workspace dependency:

```toml
jiff = { version = "0.2", features = ["serde"] }
```

## Consequences

Timestamps roundtrip losslessly, use UTC instants, and are stamped once per batch. Changing libraries requires migrating persisted events. Clock skew makes timestamps observational only; sequence defines per-stream order, while correlation and causation IDs express cross-stream causality.
