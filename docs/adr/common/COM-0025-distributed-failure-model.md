# COM-0025. Distributed Failure Model

Date: 2026-04-29
Last-reviewed: 2026-04-29
Tier: S
Status: Accepted

## Related

References: COM-0018, GND-0001, GND-0005

## Context

Cherry-pit ADRs independently address atomic file writes, optimistic concurrency, event delivery, idempotency, object stores, transport security, and stream processing. Without one common failure vocabulary, each decision can accidentally assume different retry, crash, timeout, corruption, or split-brain behavior. Distributed designs fail at the boundaries between correct local components, so the corpus needs a shared model for partial failure and recovery.

## Decision

Adopt a workspace-wide failure model for architecture decisions and implementation reviews. Components may simplify locally, but ADRs and public interfaces must state how they behave under the model where relevant.

R1 [2]: Public ports such as EventStore, EventBus, CommandBus, and CommandGateway document crash, timeout, cancellation, retry, duplicate-delivery, replay, and recovery semantics
R2 [2]: Persistent formats and stores such as EventEnvelope streams, MsgpackFileStore files, and Genome files validate corruption, version mismatch, and sequence continuity before replay
R3 [2]: Single-writer components such as MsgpackFileStore, Fiber, Dragline, and NATS stream writers reject stale writers through fencing, leases, epochs, or compare-and-swap guards
R4 [2]: Retried ingress commands carry a stable domain idempotency key handled by aggregate command handlers before new events are appended
R5 [2]: Time values such as jiff::Timestamp and UUID v7 event_id are observational metadata; stream sequence numbers define per-stream order and no global cross-stream order is inferred
R6 [2]: Recovery procedures for temp files, partial writes, stale locks, dead letters, and failed migrations are specified before the failure mode reaches production use

## Consequences

Architecture reviews have a common checklist for partial failure instead of rediscovering it per ADR. This does not require every component to become distributed or highly available; it requires every exposed boundary to state its assumptions. Some existing ADRs become implementation obligations, especially around sequence-contiguity validation, idempotent ingress, and recovery cleanup. The model deliberately rejects exactly-once claims unless backed by explicit idempotency, durable checkpoints, and fencing.
