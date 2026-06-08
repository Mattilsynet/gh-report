# PGN-0002. Fibers, Events, Timestamps, and Inspectable State

Date: 2026-06-08
Last-reviewed: 2026-06-08
Tier: B
Status: Accepted
Crates: pardosa, pardosa-wire

## Related

References: PGN-0001

## Context

Sources rescue ADR-0003 (fiber semantics) and rescue ADR-0013 (timestamps removed from the substrate). Compatible Solon material: PAR-0001 (fiber state machine encoded as an inspectable `const` data table). A `Fiber` is the unit of identity in Pardosa: `FiberId(u64)` plus a `#[non_exhaustive]` `FiberState` lifecycle, addressed by events. PAR-0001's table-encoded transition function and dot-graph generator remain valid as the implementation of the state machine; the rescue-side decisions about identity allocation, detached/precursor events, and timestamp ownership govern the substrate surface. Wall-clock removal supersedes any earlier Solon decision that placed timestamps on `Event<T>`.

## Decision

`FiberId` allocation is dragline-local: a `Dragline<T>` mints ids monotonically via its commit pipeline; there is no global registry. `FiberState` is `#[non_exhaustive]`. Each event carries `event_id`, `fiber_id`, `precursor: Option<EventId>`, `precursor_hash`, `detached: bool`, and `domain_event: T` — nothing else. Wall-clock data, if needed, lives inside `T`. Direct construction of `Fiber` values outside `Dragline::create` is unsupported. PAR-0001's `const TRANSITIONS` table is the inspectable encoding of the fiber state machine and pairs with the rescue rules below.

R1 [5]: `Dragline<T>` is the sole minter of `FiberId` and `EventId`; values are
  dragline-local, monotonic per dragline, and have no cross-process meaning
  without externally provided context.
R2 [5]: `FiberState` and `PardosaError` are `#[non_exhaustive]`; downstream
  matches must include a wildcard arm.
R3 [5]: `Event<T>` carries no `timestamp` field; producers who need wall-clock
  embed it inside `T`. No write API accepts a `ts: Timestamp` parameter.
R4 [6]: The fiber transition function is encoded as a single `const` triple
  table; runtime logic, DOT visualization, and exhaustive tests all derive
  from that table.
R5 [5]: Direct construction of `Fiber<T>` outside `Dragline::create` /
  `Dragline::create_reuse` is not supported on the public surface; tests use
  `cfg(any(test, feature = "test-support"))` helpers.

## Consequences

+ becomes easier: lock-free identity allocation; deterministic simulators that
  backdate events; HLC or registry-issued clocks layered inside `T` without
  substrate change; inspecting the fiber state machine from one table.
− becomes harder: cross-process exchange of `FiberId(7)` (requires explicit
  context); downstream consumers who joined read models on
  `(fiber_id, timestamp)` (move wall-clock into payload).
risks/migration: rescue ADR-0013 supersedes any earlier Solon decision that
  placed timestamps on the substrate; PAR-0016 (Solon timestamp policy) is
  not retired by this ADR — the retirement is deferred to a follow-up
  mission. Removing the timestamp field is a pre-publish breaking change
  recorded in CHANGELOG per PGN-0012.
