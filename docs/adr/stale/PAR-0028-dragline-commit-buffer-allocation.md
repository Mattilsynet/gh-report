# PAR-0028. Dragline Commit-Path Buffer Allocation Boundary

Date: 2026-05-20
Last-reviewed: 2026-05-20
Tier: B
Status: Accepted

## Related

References: PAR-0008, PAR-0018, PAR-0021, CHE-0064

## Context

PAR-0018 split the writer API into `reserve` / `commit` / `abandon`
to make publish-then-apply (PAR-0008) enforceable at compile time. The
v0.1 implementation carries event-bytes materialisation in
`Dragline::commit` rather than in `reserve` — visible at
`crates/pardosa/src/dragline/commit.rs:132` as a `TODO(PAR-0018)`
allocating per commit. The deferral is documented but not bounded:
without an explicit policy the allocation cost migrates downstream
unannotated and the reclaim deadline drifts. PAR-0028 freezes the
boundary: where allocation may live in v0.1, where it may not, and
what closes the gap.

## Decision

R1 [5]: `Dragline::commit` MAY allocate to materialise event bytes
  for the frontier hash (PAR-0021:R3) and the precursor hash
  (PAR-0021:R5) in v0.1. The allocation is annotated with a
  `TODO(PAR-0018)` reference at each site and counted as known
  technical debt against the reserve/commit split.

R2 [5]: `Dragline::reserve` MUST NOT allocate inside the reservation
  table beyond the bounded `ReservedEvent<T>` carrier (PAR-0018:R1).
  The reservation table itself is bounded by `max_inflight_writes`
  per PAR-0018:R5. Per-reservation auxiliary allocations are
  forbidden.

R3 [5]: `Dragline::abandon` MUST NOT allocate. Abandon is a release
  path that runs on broker NACK and must remain allocation-free so
  it cannot fail under back-pressure.

R4 [5]: The PAR-0018 reclaim work — moving event-bytes materialisation
  from `commit` to `reserve` and carrying the bytes on
  `ReservedEvent<T>` — is the v0.2 boundary. Any commit-path
  allocation introduced beyond R1's enumerated sites in v0.1
  requires an ADR amendment.

R5 [6]: Allocation cost in `commit` is benchmarked in
  `crates/pardosa/benches/` (or recorded under
  `.ooda/perf-pardosa-commit-*.md`) at v0.1 freeze. The benchmark is
  the baseline for the v0.2 reclaim work.

## Consequences

+ becomes easier: the boundary between bounded and unbounded
  allocation in the dragline writer path is named, citable, and
  testable. The v0.2 reclaim work has a clear scope.

− becomes harder: introducing a new commit-path operation that
  needs allocation (R4 requires an ADR amendment per enumerated
  site).

risks/migration: the R5 baseline benchmark must run before the v0.2
  reclaim work to confirm the regression budget. If the baseline is
  missed at v0.1 freeze, the reclaim work runs without a regression
  reference and must establish one retrospectively.
