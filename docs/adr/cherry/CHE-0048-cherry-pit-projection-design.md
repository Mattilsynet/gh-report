# CHE-0048. Cherry Pit Projection Design

Date: 2026-05-09
Last-reviewed: 2026-09-21 - amended - align current placement with canonical Cherry CPP-0001; preserve behavioral obligations and replay exemption
Tier: B
Status: Accepted

## Related

References: CHE-0005:R1, CHE-0008, CHE-0009:R1, CHE-0024:R1, CHE-0024:R3, CHE-0024:R4, CHE-0029:R4, CHE-0037:R1, CHE-0038, CHE-0053:R13, CHE-0047, CHE-0044:R3, CHE-0072, CHE-0074, CHE-0098

## Context

cherry-pit-projection is the read-side adapter driving `Projection::apply` from cherry-pit-core: pull events from an EventStore, feed a Projection in monotonic order, persist the snapshot, and checkpoint per (aggregate_id, handler) so restarts resume.

Three forces fix the shape. At-least-once persist-then-publish (CHE-0024) makes the checkpoint mandatory infrastructure. Infallible apply with no `#[non_exhaustive]` event enums and no snapshots (CHE-0009:R1, CHE-0022:R5, CHE-0037:R1) mandate full O(N) rebuild as a first-class operation. Single aggregate per port with no `Box<dyn Projection>` (CHE-0005:R1) forces compile-time generic composition — the adapter parameterises on `P: Projection` via associated types, not trait objects.

This ADR resolves five gaps as one posture bundle: storage shape, rebuild primitive, checkpoint format and location, multi-projection composition, and single-aggregate scope.

## Decision

Neutral `cherry-pit-projection` owns drivers and ephemeral views. The Pardosa-backed persistent capability lives in the outer `pardosa-cherry-pit-projection` package cohosted in canonical Cherry, per [CPP-0001](https://github.com/acje/cherry-pit/blob/ffffa0ddae206a322da9b9b4a94a3ad5e191da6e/docs/decisions/CPP-0001-outer-projection-adapter.md). This current amendment does not rewrite the immutable source snapshot cited by that decision or require a new common backend trait.

**Scope (R1–R2).** Persistent snapshot and checkpoint obligations bind the outer persistent adapter. Consumers electing replay-as-rebuild (CHE-0071), including gh-report, retain their exemption. gh-report does not currently link the outer projection adapter; this amendment makes no application-runtime adoption claim.

R1 [5]: Within `pardosa-cherry-pit-projection`, the PERSISTENT backend stores each (aggregate_id, projection_name) snapshot through Pardosa's public store/prelude facade (pgno default, nats optional; CHE-0072 selector), retaining the native DTO boundary of CHE-0074/CHE-0098. Snapshot and checkpoint are ordered effects, not an atomic pair or exactly-once guarantee.

R2 [5]: Within `pardosa-cherry-pit-projection`, the checkpoint is written strictly after the snapshot for each (aggregate_id, projection_name) pair, retaining aggregate identity, last applied sequence and handler identity, so a crash between effects causes replay rather than skipping unapplied events. Checkpoint sequence non-regression remains required.

R3 [5]: Projection::apply must be idempotent over the same EventEnvelope sequence — replaying the same monotonic sub-sequence from a checkpoint produces the same snapshot state, which is a Projection-author obligation enforced by convention and documented here rather than in the trait definition

R4 [5]: The adapter exposes a rebuild method that deletes the existing snapshot and checkpoint, loads the full event history via EventStore::load from sequence 1, replays into a fresh `P::default()`, and persists the result, making rebuild a first-class operational primitive per CHE-0047 runbook discipline

R5 [5]: The in-memory backend uses a concurrent hash map keyed by (aggregate_id, projection_name), holds no durable state, and rebuilds from the EventStore on every process start, providing the zero-dependency test and ephemeral-view backend sanctioned by CHE-0044:R3

R6 [5]: v0.1 scope is single-aggregate, single-projection-per-driver-instance — the adapter binds to one aggregate type per Projection impl via associated types (CHE-0005:R1), and multi-projection composition is deferred to the WU-5 cherry-pit-app design phase where a builder with type-state for compile-time wiring completeness will be evaluated

R7 [5]: Per-aggregate write coordination follows the single-process model inherited from CHE-0006:R1 and CHE-0053:R13, using in-process per-aggregate locks consistent with CHE-0035:R1–R3

R8 [5]: The adapter calls validate_stream() on every EventStore::load result before driving Projection::apply, per CHE-0042:R3–R4

R9 [5]: `ProjectionCheckpoint` remains a neutral cherry-pit-core carrier. Persistent stores live in the outer adapter; neutral drivers retain stream validation (R8). Neutral crates MUST NOT reverse-reexport the outer adapter or acquire Pardosa normal/build dependencies. Core's dependency budget (CHE-0029:R4) is preserved.

R10 [5]: The neutral/outer split preserves two sanctioned capabilities: EPHEMERAL (in-memory, rebuild-from-log; R5) and PERSISTENT (outer Pardosa-backed adapter; R1). Both remain first-class; consumers import the persistent adapter explicitly when required. No third backend without a new ADR.

## Consequences

The persistent-backend posture inherits the single-process locking assumption from CHE-0006:R1 and CHE-0053:R13. Multi-process projection writers would require a new ADR establishing cross-process coordination semantics — this is explicitly deferred. The PERSISTENT backend (R1) may impose additional native serialization bounds at its consumer boundary; the shared `Projection` trait and the EPHEMERAL backend's bounds (R5) are unchanged.

Single-aggregate, single-projection-per-driver scope means cross-aggregate read models (spanning bounded contexts per CHE-0005:R3) are out of scope for v0.1. Multi-projection composition deferred to WU-5 cherry-pit-app design.

Rebuild cost is O(total events per aggregate) with no snapshot shortcut (CHE-0037:R1). This bounds practical projection sizing but satisfies the schema-evolution mandate from CHE-0009:R1 + CHE-0022:R1 jointly.

## Rejected Alternatives

**SQLite (rusqlite)** — Would introduce a third storage paradigm with no CHE precedent. No existing CHE ADR cites SQLite or relational storage, and adopting it would require a separate paradigm-justification ADR plus an ongoing maintenance surface outside established patterns.

**sled embedded KV** — Same paradigm-novelty objection as SQLite, with additional ecosystem maintenance risk from reduced upstream activity.

**In-memory-only for production** — Originally rejected (2026-05) as impractical at non-trivial event volumes. RECONCILED (Layer 4, msgpack-removal-2): both production consumers (gh-report, adr-srv) ship in-memory rebuild-from-log on ephemeral Cloud Run filesystems where a local snapshot file does not survive restart. EPHEMERAL is therefore an ACCEPTED sanctioned backend (R5, R10), not a rejected alternative. The volume objection is bounded by the consumer's event count and CHE-0037:R1's no-snapshot-shortcut posture, accepted per consumer.

**Same-file checkpoint** — Bundling snapshot and checkpoint in a single MessagePack blob makes it structurally impossible to express the strict "checkpoint after snapshot" write ordering required by CHE-0024:R4. A crash during a combined write could leave a new checkpoint pointing past an incompletely written snapshot.

**Central checkpoint file** — A single workspace-wide checkpoint file keyed by (aggregate_id, projection_name) becomes a write-contention point across all projection writers, violating the per-aggregate concurrency model from CHE-0035:R2.

**object_store backend** — Forbidden until EVAL-GATE per mission boundaries and CHE-0044.

**Early multi-projection registry** — The composition decision (builder + type-state) belongs to WU-5 cherry-pit-app. Choosing a registry shape now would pre-commit an unresolved decision and risk violating CHE-0005:R1's prohibition on dynamic dispatch by requiring some form of heterogeneous projection collection.
