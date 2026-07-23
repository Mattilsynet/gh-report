# CHE-0100. Retire gateway MsgpackFileStore; nats/pgno-only persistence

Date: 2026-07-23
Last-reviewed: 2026-07-23
Tier: B
Status: Accepted
Crates: cherry-pit-gateway, cherry-pit-core

## Related

References: CHE-0098, CHE-0074, CHE-0045, CHE-0047, CHE-0032, PGN-0014 | Supersedes: CHE-0036, CHE-0043

## Context

CHE-0098:R10 retained `cherry-pit-gateway`'s `MsgpackFileStore` "solely as
the cherry-pit test-suite reference `EventStore`" after adr-srv migrated
off it, deferring full removal to "a separate future decision" (CHE-0098's
own words). That future decision has arrived: msgpack-removal-2 (Layers 1-2)
confirms gh-report's scheduler and sweep-timeout streams go ephemeral
(CHE-0099) and no production consumer of `MsgpackFileStore` remains
anywhere in the fleet. `pardosa` (native, `.pgno`-backed) and
`pardosa-nats` (JetStream-backed) are the two live durable-persistence
implementations; both satisfy CHE-0072's backend-selection contract and
CHE-0074/CHE-0098's native-store pattern. A third, retired-in-production,
msgpack-file-backed `EventStore` implementation kept alive only for test
fixtures is dead weight: it carries its own atomic-write, file-topology,
and process-fencing rules (CHE-0032:R4, CHE-0036, CHE-0043) that describe
a persistence strategy nothing in production uses.

## Decision

`cherry-pit-gateway`'s `MsgpackFileStore` is retired in full — code,
`rmp-serde` dependency, and its dedicated file-topology and file-fencing
ADRs. `pardosa` (`.pgno`) and `pardosa-nats` (JetStream) are ratified as
the only durable persistence implementations fleet-wide.

R1 [5]: `pardosa` (`.pgno`-backed) and `pardosa-nats` (JetStream-backed) are the only durable-persistence `EventStore` implementations fleet-wide; msgpack-file persistence (`MsgpackFileStore`) is retired. `cherry-pit-core`'s downstream test/fixture cleanup follows from this rule directly — no separate ADR governs it.
R2 [5]: `cherry-pit-gateway` deletes `MsgpackFileStore` (module, type, and `StoreError` variants unique to it) and drops the `rmp-serde` dependency once its test consumers migrate per R3.
R3 [5]: Each `MsgpackFileStore` test consumer migrates to a pgno-backed `EventStore` where the test asserts file, durability, or crash-recovery semantics, or to an in-memory `EventStore` where the test only exercises `EventStore` trait behaviour; the choice per test site is triaged by copernicus evidence, not by this ADR.
R4 [5]: CHE-0036 (file-per-stream full-rewrite storage model) and CHE-0043 (process-level file fencing) are superseded by this ADR: no rule in either survives `MsgpackFileStore` deletion, because both describe topology and fencing specific to that store's on-disk layout, not to pgno's differing topology (fencing for pgno is PGN-0014's concern).
R5 [5]: CHE-0032's atomic-write protocol (R1-R3: temp-file + fsync + rename + parent-dir fsync) is retained as a general crash-safety pattern; only its `MsgpackFileStore`-specific exemplar (R4) is retired, since the store it names no longer exists.
R6 [5]: CHE-0036 and CHE-0043 move to `docs/adr/stale/` with `Status: Superseded`, superseded-by this ADR.

## Consequences

+ becomes easier: one persistence story fleet-wide (pgno + NATS), no
  dormant test-only store carrying its own topology/fencing ADRs.
+ becomes easier: `cherry-pit-gateway`'s dependency footprint drops
  `rmp-serde`.
− becomes harder: no `pgno`-backed `cherry_pit_core::EventStore` adapter
  exists yet. L2-CODE MUST build a thin pgno-backed test adapter, or
  wire tests to `pardosa`/`pardosa-nats` directly where file/durability
  semantics are asserted; trait-only tests move to in-memory. This ADR
  ratifies the destination, not the adapter code.
risks/migration: CHE-0032's R1-R3 protocol has no live pgno exemplar
  at authoring time — no `pardosa*` crate implements temp-file+rename
  yet (checked `pardosa-file/src/{writer,manifest/writer}.rs`; both
  append/WAL-style). CHE-0053:R5-R6 already separate mechanism from
  invariant; a future pgno atomic-write site cites CHE-0032 directly.
