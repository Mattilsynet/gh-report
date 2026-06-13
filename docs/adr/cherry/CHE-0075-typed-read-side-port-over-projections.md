# CHE-0075. Typed Read-Side Port Over Projections

Date: 2026-06-13
Last-reviewed: 2026-06-13
Tier: B
Status: Proposed

## Related

References: CHE-0013, CHE-0050, CHE-0048, CHE-0005, CHE-0025, CHE-0024, CHE-0001

## Context

CHE-0013 sends reads through projections but leaves no typed read contract.
`Projection` already folds typed events into a query model
(`crates/cherry-pit-core/src/projection.rs:41-49`), and `adr-srv`
currently reads directly from `AdrCorpus` with no live read push slot
(`crates/adr-srv/src/graphql/mod.rs:35-41`). The gap is the read-side
counterpart to CHE-0050's compile-time port discipline.

## Decision

Adopt a thin typed read-side port over projection state. P1 correctness
wins over response-time convenience: the compiler must constrain read
wiring the same way it constrains write wiring.

R1 [5]: Expose one typed read-side port whose associated projection and query types bind each instance to a single aggregate read model

R2 [5]: Resolve queries from projection state only; never load write-side history or dispatch commands from the read port

R3 [5]: Use RPITIT for async infrastructure methods, matching the existing port discipline without per-call boxed futures

R4 [5]: Use associated types and generic state parameters so implementations cannot sit behind type-erased registries

R5 [6]: Let consumers name query and response DTOs while the substrate enforces read-only single-aggregate projection access

R6 [5]: Defer live read-model subscription surfaces; v0.1 change signalling remains persist-then-publish plus checkpointed replay

R7 [5]: Keep core carriers within the existing dependency budget; adapter crates own transport-specific query exposure

R8 [5]: Reject scatter-gather, location-transparent, or dynamically registered read fabrics as outside the static wiring contract

## Consequences

+ becomes easier: agents can target reads through a named typed port rather
  than open-coded projection access, preserving single-aggregate compile-time
  constraints on both sides of the system.

− becomes harder: each consumer must implement the small query translation
  boundary for its own projection DTOs instead of relying on a shared dynamic
  registry.

risks/migration: existing direct projection reads remain valid until the
  Proposed rule is ratified and implemented. Live read-model push is a tracked
  deferral folded into this ADR; a concrete consumer need must justify it later.
