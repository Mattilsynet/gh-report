# SEC-0006. Eliminate Race Conditions by Construction

Date: 2026-04-28
Last-reviewed: 2026-04-28
Tier: A
Status: Accepted

## Related

References: SEC-0001

## Context

TOCTOU (time-of-check to time-of-use) and other race conditions
occur when a check and its dependent action are non-atomic and
external state can change between them. The CISQ model classifies
this as a composition of Integrity and Control threats. Prevention
requires making check-then-act sequences atomic or eliminating the
temporal gap between check and use entirely.

## Decision

Eliminate race conditions by making check-then-act sequences
atomic or by removing the temporal gap through construction.

R1 [4]: File creation uses atomic temp-file-then-rename; never
  check-then-open sequences
R2 [4]: Single-writer-per-stream eliminates concurrent mutation
  at the architectural level, not through runtime locks
R3 [5]: Prefer EAFP (ask forgiveness) over LBYL (look before you
  leap) — attempt the operation and handle failure, rather than
  checking preconditions that may become stale
R4 [5]: Fencing tokens or monotonic sequence numbers guard
  ownership claims across process restarts

## Consequences

Race conditions are prevented by structure, not by hoping for
correct timing. Atomic file operations prevent data corruption from
interrupted writes. Single-writer architecture eliminates concurrent
mutation entirely for write paths. The trade-off is reduced write
concurrency, which is acceptable for correctness-first systems.
