# CHE-0075. Typed Read-Side Port Over Projections

Date: 2026-06-13
Last-reviewed: 2026-06-15
Tier: B
Status: Accepted

## Related

References: CHE-0013, CHE-0050, CHE-0048, CHE-0005, CHE-0025, CHE-0024, CHE-0001

## Context

CHE-0013 sends reads through projections but leaves no typed read contract.
`Projection` already folds typed events into a query model, and the
read-side consumers need the same compile-time port discipline CHE-0050 gives
the write side. `adr-srv` resolves GraphQL reads from `AdrCorpus`; `gh-report`
renders reports from `EvidenceProjection`. Both reads are synchronous reads of
already-materialised state guarded by their existing in-process locks.

## Decision

Adopt a thin typed read-side port over projection state. P1 correctness
wins over response-time convenience: the compiler must constrain read
wiring the same way it constrains write wiring.

R1 [5]: Expose one typed read-side port in `cherry-pit-core` whose associated projection-state, query, and response types bind each implementation to one read model

R2 [5]: Resolve queries from projection state only; never load write-side history or dispatch commands from the read port

R3 [5]: Keep v0.1 `resolve` synchronous; if a later read infrastructure method becomes asynchronous, use RPITIT without per-call boxed futures

R4 [5]: Use associated types and an associated `resolve` function so implementations cannot sit behind type-erased registries

R5 [6]: Let consumers name query and response DTOs while the substrate enforces read-only single-aggregate projection access

R6 [5]: Defer live read-model subscription surfaces; v0.1 change signalling remains persist-then-publish plus checkpointed replay

R7 [5]: Keep `ReadPort` in `cherry-pit-core` with zero new dependencies; adapter crates own transport-specific query exposure and DTOs

R8 [5]: Reject scatter-gather, location-transparent, or dynamically registered read fabrics as outside the static wiring contract

## Consequences

+ becomes easier: agents can target reads through a named typed port rather
  than open-coded projection access, preserving single-aggregate compile-time
  constraints on both sides of the system.

− becomes harder: each consumer must implement the small query translation
  boundary for its own projection DTOs instead of relying on a shared dynamic
  registry.

risks/migration: existing projection methods remain valid for local projection
  internals, while cross-consumer read wiring routes through `ReadPort`.
  Live read-model push is a tracked deferral folded into this ADR; a concrete
  consumer need must justify it later.
