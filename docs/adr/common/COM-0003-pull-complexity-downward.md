# COM-0003. Pull Complexity Downward

Date: 2026-04-25
Last-reviewed: 2026-04-28
Tier: B
Status: Accepted

## Related

References: COM-0002

## Context

Ousterhout (Ch. 8) argues that since a module is implemented once but called many times, complexity in the implementation is paid once while complexity in the interface is paid by every caller. Configuration parameters are a specific case — each option pushes a decision to callers who must understand and choose correctly.

Cherry-pit applies this extensively: the store creates envelopes (CHE-0016) so callers pass only `Vec<Event>`; infrastructure owns identity (CHE-0020); two-level concurrency (CHE-0035) hides locks behind three methods; file fencing (CHE-0043) acquires lazily without caller involvement.

## Decision

When complexity cannot be eliminated, pull it into the
implementation rather than exposing it through the interface.

R1 [5]: Callers pass minimal information; if the module can compute,
  derive, or default a value, the caller must not provide it
R2 [5]: Every configuration parameter requires justification
  demonstrating the module cannot determine the value automatically
R3 [5]: Configuration parameters must have sensible defaults that
  produce correct behavior without caller configuration
R4 [6]: When a module can handle an error internally through retry,
  fallback, or default, it must not propagate the error to the caller
R5 [6]: When a module absorbs an error internally, emit a structured
  log entry or metric at the absorption point so failure patterns
  remain observable

## Consequences

Infrastructure implementations absorb operational complexity: the `EventStore` trait has 3 methods while `MsgpackFileStore` has 600+ lines handling concurrency, fencing, atomic writes, and serialization. New infrastructure ports are evaluated for interface simplicity — proposals requiring callers to manage locks or retry logic are challenged to pull that complexity down. Tension with transparency is mitigated by structured logging and detailed error messages that expose internal state during failures, not during normal operation.
