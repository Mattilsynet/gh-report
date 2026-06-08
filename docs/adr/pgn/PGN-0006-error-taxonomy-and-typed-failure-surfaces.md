# PGN-0006. Error Taxonomy and Typed Failure Surfaces

Date: 2026-06-08
Last-reviewed: 2026-06-08
Tier: B
Status: Accepted
Crates: pardosa, pardosa-wire, pardosa-file

## Related

References: PGN-0001, PGN-0003, PGN-0004

## Context

Sources rescue ADR-0007 (error taxonomy) primarily; rescue ADR-0021 contributes anchor-side error variants framed for the eventual implementation. Errors cross three rings and must respect ring direction (PGN-0001). Substrate (`pardosa-wire::DecodeError`, `pardosa-file::FileError`) is `thiserror`-free; runtime (`pardosa::PardosaError`, `persist::Error`) uses `thiserror`. The 2026-05-24 amendment fixes a wrapping cycle: `PardosaError → persist::Error → PardosaError` was broken via an operation-scoped projection enum at the boundary.

## Decision

Substrate errors are handcrafted, `Display`-stable, and `thiserror`-free. Runtime errors use `thiserror` and may compose substrate errors via `#[from]` / `#[source]`. Every public error enum is `#[non_exhaustive]`. Schema-hash mismatch and tamper-detection variants are typed first-class — not generic IO. The runtime error graph is acyclic: `PardosaError` may wrap `persist::Error` (one-way) but `persist::Error` must not wrap `PardosaError`; operation-scoped projection kinds at the boundary preserve the diagnostic chain without cycles.

R1 [5]: Every public error enum in the workspace is `#[non_exhaustive]` from
  day one; downstream `match` arms must include a wildcard arm.
R2 [5]: Substrate errors are handcrafted enum + `impl Display`/`impl Error`,
  with no `thiserror` dependency; runtime errors use `thiserror` and may
  carry substrate errors via `#[from]` or `#[source]`.
R3 [5]: Schema-hash mismatch surfaces as a typed first-class variant
  (e.g. `persist::Error::SchemaHashMismatch { expected, found }`) *before*
  any payload byte is decoded.
R4 [5]: Per-message-checksum and footer-checksum mismatches surface as
  distinct `FileError` variants — never as a generic `Io` failure — so
  consumers can match on them.
R5 [5]: The runtime error graph is acyclic: `PardosaError` may wrap
  `persist::Error` one-way; `persist::Error` must not wrap `PardosaError`.
R6 [5]: When a module's API exposes an invariant violation from the root
  error type, the public variant carries an operation-scoped projection
  enum, not the root error, with `From<RootError>` implementing the
  projection exhaustively.
R7 [5]: Any future anchor-verification surface uses its own
  `#[non_exhaustive]` enum distinct from `FileError` and never silently
  coerces an authenticator failure into unanchored success.

## Consequences

+ becomes easier: principled error recovery (delete sidecar vs do-not-delete
  vs drop-and-reopen vs cannot-recover); preserved diagnostic chains
  through `#[source]`; bounded blast radius via projection at boundaries.
− becomes harder: every downstream `match` carries a wildcard arm
  (`#[non_exhaustive]`); substrate error variants are more verbose than
  the `thiserror`-equivalent.
risks/migration: removing or renaming a public error variant is a
  pre-publish breaking change per PGN-0009 / PGN-0012; the cycle break
  in 2026-05-24 (F2) is binding and must not be re-introduced.
