# CHE-0102. Server-Authoritative Backoff Regulator for cherry-pit-wq (Inherit CHE-0046, Reconcile FLO-0009:R1)

Date: 2026-07-31
Last-reviewed: 2026-07-31

Tier: B
Status: Accepted
Crates: cherry-pit-wq, gh-report

## Related

References: CHE-0055, CHE-0101, CHE-0046, CHE-0095, CHE-0084, FLO-0009, CHE-0018, CHE-0021, CHE-0025

## Context

gh-report has no secondary-rate-limit / `Retry-After` handling (mission
ghr-16813e99, copernicus survey ghr-bede5372): `Retry-After` is parsed
nowhere; 429/503 responses get identical blind jittered-exponential
backoff; a 403-with-secondary-limit-abuse-detection body is
indistinguishable from a plain 403-permission-denied. Repeatedly hitting
GitHub's secondary rate limit escalates the abuse-detection lockout —
outage-class risk if the server's authoritative resume-at signal is
under-honored (resumed too early). This ADR ratifies the design for a
domain-agnostic pause-until-instant regulator on the `cherry-pit-wq`
`Regulator` seam (CHE-0055 R10/R16 addenda), plus the gh-report-side HTTP
adapter that feeds it, and reconciles the one anticipated tension:
FLO-0009:R1's continuous-admission-gradient preference against a
`Retry-After` pause (oracle ruling ghr-a02854a2).

## Decision

Ratify `BackoffRegulator` (cherry-pit-wq) as a `Regulator` that pauses
worker admission until a caller-supplied resume-at `Instant`, and ratify
the gh-report-side secondary-limit parsing that feeds it.

R1 [5]: The `BackoffRegulator` pauses worker admission until a
  caller-supplied resume-at `Instant`, then resumes; it is domain-agnostic
  (opaque `Instant`, no HTTP/GitHub vocabulary per CHE-0084:R1/R7/R9). It
  implements the existing `Regulator` trait via park-then-admit (the
  `RateLimitRegulator::admit` precedent) — no `Admission` enum change.
  `settle` is a no-op; a backoff pause carries no charge concept.

R2 [5]: The regulator INHERITS CHE-0046 and does not replace it — retry
  stays gated on `ErrorCategory`-equivalent retryability (CHE-0046:R1:
  gh-report's `ApiOutcome::is_retryable`), bounded-attempts + total-deadline
  + jitter (CHE-0046:R1), idempotency-keyed (CHE-0046:R3), retry telemetry
  (CHE-0046:R6); cancellation ≠ rollback (CHE-0046:R5). Cite CHE-0095:R5's
  inherit-not-replace shape as the precedent for this inheritance.

R3 [5]: A server-specified `Retry-After` OVERRIDES the computed
  exponential wait for that single wait; absent it, the client falls back
  to CHE-0046 jittered-exponential bounded by the existing attempt cap. The
  override narrows the wait only; bounded-attempts and total-deadline are
  unaffected. It MUST NEVER shorten the server's stated wait: parsing
  rounds UP, and `set_backoff` uses `fetch_max` so a later, shorter
  observation cannot regress an armed resume-at (the outage-class safety
  rule — under-honoring escalates GitHub's abuse-detection lockout).

R4 [4]: FLO-0009:R1 reconciliation — a `Retry-After` pause is a FEEDBACK
  response to a server-authoritative signal, not a speculative binary gate
  on local queue-fill. FLO-0009:R1's gradient preference governs
  self-imposed admission on local saturation; an upstream `Retry-After`
  gives the exact resume instant, so there is no gradient to shape. Same
  class as the primary-limit `halted_until` pause — upstream backpressure
  obeyed, not a self-imposed breaker trip. PAR-0014's binary breaker is
  non-binding: no local open/closed trip on failure counts is built.

R5 [4]: Ring split — the generic `BackoffRegulator` lives in
  `cherry-pit-wq` (opaque `Instant`, no runtime construction, CHE-0055:R5).
  HTTP parsing (429, 403 secondary-limit marker, `Retry-After`
  seconds-or-HTTP-date) lives in gh-report, mirroring the concrete
  `halted_until` pattern rather than the aspirational `RateLimitObserver`
  trait. Respects CHE-0018:R1-R3 and CHE-0007/CHE-0029 (no GitHub/HTTP
  crate enters `cherry-pit-wq`).

R6 [5]: Any new error/outcome enum on the wq surface carries
  `#[non_exhaustive]` (CHE-0021); `Regulator` methods stay
  sync / RPITIT, never `#[async_trait]` (CHE-0025).

R7 [3]: A local circuit breaker / bulkhead is deferred (trigger-gated
  NOT-YET per CHE-0037/CHE-0040 deferral precedent; compose-not-monolith —
  the `Regulator` seam composes small regulators, not a monolith). PAR-0014
  is non-binding for this design (R4). Building a breaker/bulkhead here is
  explicitly out of scope of this ADR.

## Consequences

+ becomes easier: the collector honors GitHub's secondary-limit signal
  exactly, closing the founding F3 gap.
+ becomes easier: a future breaker/bulkhead has a settled precedent for
  composing onto the same seam (R7) without re-litigating FLO-0009:R1.
− becomes harder: a caller must know `self.backoff`'s shared,
  never-shortened resume-at applies to every concurrent retryable failure
  on the client, not just the one that armed it (deliberate, R3).
risks/migration: additive only — remove `BackoffRegulator` from the chain
  to revert to the pre-mission gap. No frozen path is touched.
