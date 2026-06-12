# CHE-0070. ListableEventStore Async Signature

Date: 2026-06-10
Last-reviewed: 2026-06-10
Tier: B
Status: Accepted

## Related

References: CHE-0057, CHE-0018, CHE-0048, CHE-0074 | Supersedes: CHE-0067

## Context

CHE-0067 ratified `ListableEventStore::list_aggregates` with a
synchronous signature; CHE-0067:R5 fixed it as append-only,
requiring a superseding ADR for any change. Findings F11 (bd
`adr-fmt-cq7vb.6`) surfaced the resulting reactor-stall hazard:
`MsgpackFileStore::list_aggregates` calls blocking `std::fs::read_dir`,
stalling the tokio reactor when invoked from `async fn` callers
(`bootstrap_replay_state`, `new_with_replay`). CHE-0018:R2 binds every
infrastructure port to async; CHE-0067 was the only divergent
method. The inherent-helper workaround silently violates CHE-0067:R4
(downstream MUST bound on the trait). The remaining shape is a
superseding ADR per CHE-0057:R5 that aligns R5 with CHE-0018:R2.

## Decision

`ListableEventStore::list_aggregates` is async. The signature follows
the idiomatic RPITIT shape used by every other infrastructure-port
method in cherry-pit-core (`EventStore::load/create/append`,
`HashChainedEventStore::verify_chain`, `EventBus::publish`). R1–R4
carry forward from CHE-0067 unchanged; R5 substantively changes to
specify the async signature; R6 governs implementation conditions for
blocking substrate calls. cherry-pit-core acquires no async-runtime
dependency: `impl Future + Send` is `core::future::Future`, consistent
with CHE-0018:R3.

R1 [5]: `ListableEventStore` extends `EventStore` as a supertrait
  bound per CHE-0057:R2, lives in `cherry-pit-core` alongside
  `EventStore`, and is named per CHE-0057:R2's
  `<Capability>EventStore` convention.

R2 [5]: The trait surface is the single method `list_aggregates(&self)
  -> impl Future<Output = Result<Vec<AggregateId>, StoreError>> +
  Send` returning every known `AggregateId` in unspecified order. An
  empty store returns `Ok(vec![])`, never `Err`; errors reflect
  substrate I/O failure (`StoreError::Infrastructure`) or task-join
  failure (`StoreError::JoinFailure`) only.

R3 [5]: File-backed `EventStore` implementations that enumerate
  cheaply from local indexes (e.g. gh-report's native pardosa store port
  per CHE-0074) MUST implement `ListableEventStore`. In-process stores whose stream
  map is enumerable in `O(stream-count)` MUST implement it. Remote or
  otherwise non-enumerable substrates MUST NOT implement it per
  CHE-0057:R3.

R4 [5]: Downstream code requiring enumeration MUST bound on
  `ListableEventStore` per CHE-0057:R4, not on `EventStore`. Trait
  objects (`dyn ListableEventStore`) are forbidden, preserving
  CHE-0005:R1 single-aggregate-per-port binding across the extension.

R5 [5]: The `list_aggregates` signature is async per CHE-0018:R2 and
  append-only per CHE-0057:R5; adding methods or changing the return
  shape requires a superseding ADR. Returning
  `Err(NotImplemented)` from `list_aggregates` is forbidden;
  substrates that cannot enumerate MUST omit the impl.

R6 [5]: Implementations whose enumeration calls blocking syscalls
  (e.g. `std::fs::read_dir`) MUST wrap the blocking work in
  `tokio::task::spawn_blocking` and map the resulting
  `tokio::task::JoinError` to `StoreError::JoinFailure`. In-memory
  implementations whose body is non-blocking and holds no `.await`
  point MAY use `async fn` sugar over a synchronous body without
  `spawn_blocking`.

## Consequences

+ becomes easier: async callers no longer block the tokio reactor
  during enumeration; the trait shape matches every other
  cherry-pit-core infrastructure port (auditable boundary per
  CHE-0018); callers can distinguish substrate I/O failure
  (`Infrastructure`) from runtime/task-shutdown failure
  (`JoinFailure`).

− becomes harder: synchronous-only consumers must `.await` or
  `block_on`; exhaustive matches on `StoreError` must add a
  `JoinFailure` arm — mitigated by `#[non_exhaustive]` (CHE-0021:R1)
  on `StoreError`.

risks/migration: bd `adr-fmt-cq7vb.6` (sub-mission E-2) lands the
  signature change in one squash-set: trait + 2 impls + 5 call sites
  + new `StoreError::JoinFailure` variant + reactor-stall regression
  test. No `dyn ListableEventStore` exists per pre-flight grep. CHE-0067
  retires to `stale/` per AFM-0022 in the same commit as this ADR.
