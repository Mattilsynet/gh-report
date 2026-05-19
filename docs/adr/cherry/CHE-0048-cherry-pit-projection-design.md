# CHE-0048. Cherry Pit Projection Design

Date: 2026-05-09
Last-reviewed: 2026-05-19
Tier: B
Status: Accepted

## Related

References: CHE-0005:R1, CHE-0008, CHE-0009:R1, CHE-0024:R1, CHE-0024:R3, CHE-0024:R4, CHE-0029:R4, CHE-0036, CHE-0037:R1, CHE-0038, CHE-0043, CHE-0047, CHE-0044:R3, CHE-0065

## Context

cherry-pit-projection is the read-side adapter driving `Projection::apply` from cherry-pit-core: pull events from an EventStore, feed a Projection in monotonic order, persist the snapshot, and checkpoint per (aggregate_id, handler) so restarts resume.

Three forces fix the shape. At-least-once persist-then-publish (CHE-0024) makes the checkpoint mandatory infrastructure. Infallible apply with no `#[non_exhaustive]` event enums and no snapshots (CHE-0009:R1, CHE-0022:R5, CHE-0037:R1) mandate full O(N) rebuild as a first-class operation. Single aggregate per port with no `Box<dyn Projection>` (CHE-0005:R1) forces compile-time generic composition — the adapter parameterises on `P: Projection` via associated types, not trait objects.

This ADR resolves five gaps as one posture bundle: storage shape, rebuild primitive, checkpoint format and location, multi-projection composition, and single-aggregate scope.

## Decision

The projection storage adapter uses file-based MessagePack snapshots as the production backend, with an in-memory backend for tests and ephemeral views. Both implement a single internal port trait parameterised on `P: Projection`.

**Scope (R1–R2).** The on-disk snapshot + checkpoint persistence mandate in R1 and R2 binds the `cherry-pit-projection` crate — the canonical projection runtime. Consumers that elect replay-as-rebuild per CHE-0065 (in-memory projections rebuilt by full event replay per CHE-0037:R1) are exempt; `gh-report` is the v0.1 instance of this election, retiring `baseline.msgpack` and `checkpoint` files in favour of event-log replay. The exemption is a scope reduction, not a content reversal: R1–R2 remain binding for cherry-pit-projection.

R1 [5]: Within `cherry-pit-projection`, the production adapter writes one MessagePack file per (aggregate_id, projection_name) tuple using rmp-serde with named encoding, following the temp-file-then-rename atomicity pattern established by CHE-0032:R1–R4 and the advisory `.lock` fencing model from CHE-0043:R1–R3

R2 [5]: Within `cherry-pit-projection`, a sibling checkpoint file is written strictly after the snapshot file for each (aggregate_id, projection_name) pair, recording aggregate_id, last applied sequence (NonZeroU64), and handler identity string, so that a crash between snapshot and checkpoint causes replay of already-applied events rather than skipping unapplied ones

R3 [5]: Projection::apply must be idempotent over the same EventEnvelope sequence — replaying the same monotonic sub-sequence from a checkpoint produces the same snapshot state, which is a Projection-author obligation enforced by convention and documented here rather than in the trait definition

R4 [5]: The adapter exposes a rebuild method that deletes the existing snapshot and checkpoint, loads the full event history via EventStore::load from sequence 1, replays into a fresh `P::default()`, and persists the result, making rebuild a first-class operational primitive per CHE-0047 runbook discipline

R5 [5]: The in-memory backend uses a concurrent hash map keyed by (aggregate_id, projection_name), holds no durable state, and rebuilds from the EventStore on every process start, providing the zero-dependency test and ephemeral-view backend sanctioned by CHE-0044:R3

R6 [5]: v0.1 scope is single-aggregate, single-projection-per-driver-instance — the adapter binds to one aggregate type per Projection impl via associated types (CHE-0005:R1), and multi-projection composition is deferred to the WU-5 cherry-pit-agent design phase where a builder with type-state for compile-time wiring completeness will be evaluated

R7 [5]: Per-aggregate write coordination follows the single-process model inherited from CHE-0006:R1 and CHE-0043, using in-process per-aggregate locks consistent with CHE-0035:R1–R3

R8 [5]: The adapter calls validate_stream() on every EventStore::load result before driving Projection::apply, per CHE-0042:R3–R4

R9 [5]: The `ProjectionCheckpoint` data type lives in cherry-pit-core as a peer of `EventEnvelope` (CHE-0042) — both are core-resident serde-deriving carriers — while `FileProjectionStore` and identity validation (R8) remain in cherry-pit-projection, which re-exports `ProjectionCheckpoint` for back-compat; core's dep budget (CHE-0029:R4) is preserved with no new dependencies

## Consequences

The file-based posture inherits the single-process locking assumption from CHE-0006:R1 and CHE-0043. Multi-process projection writers would require a new ADR establishing cross-process coordination semantics — this is explicitly deferred.

Single-aggregate, single-projection-per-driver scope means cross-aggregate read models (spanning bounded contexts per CHE-0005:R3) are out of scope for v0.1. Multi-projection composition deferred to WU-5 cherry-pit-agent design.

Rebuild cost is O(total events per aggregate) with no snapshot shortcut (CHE-0037:R1). This bounds practical projection sizing but satisfies the schema-evolution mandate from CHE-0009:R1 + CHE-0022:R1 jointly.

## Rejected Alternatives

**SQLite (rusqlite)** — Would introduce a third storage paradigm with no CHE precedent. No existing CHE ADR cites SQLite or relational storage, and adopting it would require a separate paradigm-justification ADR plus an ongoing maintenance surface outside established patterns.

**sled embedded KV** — Same paradigm-novelty objection as SQLite, with additional ecosystem maintenance risk from reduced upstream activity.

**In-memory-only for production** — Acceptable for tests and ephemeral views (shipped as the companion backend per R5) but not viable as the sole adapter because rebuild-from-zero on every process start is impractical at non-trivial event volumes.

**Same-file checkpoint** — Bundling snapshot and checkpoint in a single MessagePack blob makes it structurally impossible to express the strict "checkpoint after snapshot" write ordering required by CHE-0024:R4. A crash during a combined write could leave a new checkpoint pointing past an incompletely written snapshot.

**Central checkpoint file** — A single workspace-wide checkpoint file keyed by (aggregate_id, projection_name) becomes a write-contention point across all projection writers, violating the per-aggregate concurrency model from CHE-0035:R2.

**object_store backend** — Forbidden until EVAL-GATE per mission boundaries and CHE-0044.

**Early multi-projection registry** — The composition decision (builder + type-state) belongs to WU-5 cherry-pit-agent. Choosing a registry shape now would pre-commit an unresolved decision and risk violating CHE-0005:R1's prohibition on dynamic dispatch by requiring some form of heterogeneous projection collection.
