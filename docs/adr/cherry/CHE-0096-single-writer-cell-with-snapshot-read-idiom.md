# CHE-0096. Single-Writer Cell With Snapshot-Read Idiom

Date: 2026-07-22
Last-reviewed: 2026-07-22
Tier: B
Status: Accepted

## Related

References: CHE-0048, CHE-0075, COM-0018, CHE-0009 | Supersedes: none

## Context

Oracle review (adr-fmt-41zs8 A1) found a load-bearing tension lifting
gh-report's `Arc<Mutex<P>>` projection-state helpers
(`app/state.rs:823-870`) into `cherry-pit-projection`: CHE-0048:R7 permits
an in-process per-aggregate lock as write-coordination, but COM-0018:R3
requires shared reads to use immutable snapshots, not read-write lock
sharing. Exporting the Mutex as a shared cell that hands out `MutexGuard`
reads is ADR-illegal. This ADR names the reconciled idiom: single-owner
write cell, snapshot-yielding read, caller-supplied event identity.

## Decision

`cherry-pit-projection` MAY offer a generic single-writer cell over
`P: Projection` reconciling CHE-0048:R7's in-process lock with COM-0018:R3's
snapshot-read requirement.

R1 [5]: The cell's write path MUST serialize mutation through an
  in-process lock (CHE-0048:R7); the lock MUST NOT be exposed on the
  public surface, and no public method may return a lock guard.

R2 [5]: The cell's read path MUST return an owned or `Arc`-shared
  immutable snapshot (COM-0018:R3) resolved via CHE-0075:R1-R2 typed
  read access; mutating the returned snapshot MUST NOT affect the cell.

R3 [5]: The cell's write method MUST accept a caller-constructed
  `EventEnvelope<P::Event>`; it MUST NOT synthesize event identity
  (sequence, aggregate id, timestamp) — that remains the caller's
  responsibility per the aggregate's own identity contract.

R4 [5]: `Projection::apply` (CHE-0009:R1) stays infallible and total;
  the cell's write method MUST NOT introduce a fallible fold path beyond
  what `Projection::apply` itself defines.

## Consequences

+ becomes easier: consumers get a substrate-legal single-writer cell
  without re-deriving the CHE-0048:R7/COM-0018:R3 reconciliation per call
  site; the type system forecloses lock-guard leakage.
− becomes harder: callers must construct their own `EventEnvelope`
  (sequence, identity) before calling the write method — no cell-side
  convenience synthesis.
risks/migration: existing gh-report `lock_projection`/`resolve_projection`
  call sites that read via the returned `MutexGuard` are pre-existing usage
  and stay valid; new consumers of the extracted cell get the
  snapshot-read shape by construction.
