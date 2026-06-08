# PAR-0016. Timestamp Policy and Cross-Stream Ordering

Date: 2026-04-28
Last-reviewed: 2026-04-29
Tier: B
Status: Accepted

## Related

References: PAR-0004, COM-0025

## Context

PAR-0004 states wall-clock timestamp suffices for single-writer ordering.
Within one stream, monotonic event_id (PAR-0007) provides true order.
Across streams, wall-clock is unreliable: NTP drift, VM migration, leap
seconds. Lamport (1978) establishes wall-clock is insufficient for causal
ordering across independent writers.

1. **Advisory timestamps** — informational only; cross-stream has no
   global order and joins use domain correlation keys.
2. **Hybrid Logical Clocks** — causal ordering, 8-byte overhead per event.
3. **Vector clocks** — full causal history, overkill for single-writer.

Option 1 chosen: cross-stream consumers compare JetStream sequence
numbers only within the same stream. HLC deferred until multi-writer is
required.

## Decision

Timestamps are advisory metadata — not a causal ordering mechanism.
There is no global cross-stream order. Cross-stream event correlation
uses domain-level keys and causation metadata, while JetStream sequence
numbers are compared only within each individual stream.

R1 [6]: Persist timestamps via jiff::Timestamp::now() as advisory
  metadata in EventEnvelope, documented as non-causal across streams
R2 [5]: Per-stream event ordering uses JetStream-provided stream
  sequence numbers accessed through the consumer API
R3 [6]: Consumers correlating events across multiple streams compare
  JetStream sequences within each stream independently
R4 [6]: Cross-stream projections use domain identifiers,
  correlation_id, causation_id, or explicit join keys rather than
  timestamp or sequence equality

## Consequences

False cross-stream ordering assumptions are eliminated without adding HLC or vector-clock overhead. Timestamps remain useful for audit and approximate windows. Cross-stream correlation joins on domain keys or explicit causality fields.
