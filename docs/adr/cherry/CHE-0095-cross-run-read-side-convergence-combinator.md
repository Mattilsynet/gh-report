# CHE-0095. Cross-Run Read-Side Convergence Combinator (Ratify Boundary, Defer Port)

Date: 2026-07-22
Last-reviewed: 2026-07-22
Tier: B
Status: Accepted

## Related

References: CHE-0046, CHE-0088, PGN-0016, PGN-0023, CHE-0037, CHE-0040 | Supersedes: none

## Context

gh-report's `converge_on_fence` (`crates/gh-report/src/app/daemon.rs`)
already implements a cross-run, closure-generic, policy-parameterised
resync-then-retry combinator after an OCC fence conflict, cited by
CHE-0088:R9/R10 as the sanctioned sink. It stays gh-report-local: its retry
policy is a fixed constant, not caller-supplied, and no cherry-pit ADR
states the contract this idiom must obey if lifted. Oracle review
(adr-fmt-a47kk) found the boundary already ratified: PGN-0023:R4 blesses a
read-tier catch-up/RYW fence; PGN-0023:R5 forbids that shape on the append
path, citing PGN-0016:R10's ban on tip-resync-then-retry inside append. This
ADR names the combinator CHE-0046 extends, and states the constraints a
future port MUST carry so the idiom is not smuggled in as mere "convergence
is allowed" convenience. Feynman orientation (adr-fmt-xxr3u) found
extraction premature at n=1 (COM-0001:R2): no second consumer exists to
triangulate policy shape. This ADR ratifies the boundary only; the port
stays deferred per the CHE-0037/CHE-0040 trigger-gated precedent.

## Decision

A cross-run, read-side convergence combinator is ratified substrate
doctrine, extending CHE-0046's bounded/`ErrorCategory::Retryable`-gated/
idempotency-keyed retry contract with a resync step. The port
implementation is deferred until a genuine second cherry-pit consumer
exists; gh-report's `converge_on_fence` remains the sole, gh-report-owned
instance until that trigger fires.

R1 [5]: A convergence combinator MAY re-attempt a command only as a
  cross-run writer that already aborted the losing run and re-established
  ownership by replaying authoritative state (PGN-0016:R2/R7); it MUST NOT
  tip-resync and retry inside the same append attempt (PGN-0016:R10) — the
  read/append boundary PGN-0023:R5 draws applies unchanged here.

R2 [5]: The resync step of any convergence combinator MUST re-read
  authoritative state exclusively through a typed read-side port
  (CHE-0075:R1-R2) resolving from projection state, never from write-side
  history or a command dispatch path; this is the type-level fence that
  keeps the combinator on the read side of PGN-0023:R5's boundary.

R3 [5]: Any convergence combinator's retry/backoff policy MUST be a
  caller-supplied type parameter, not a fixed constant baked into the
  combinator; gh-report's current fixed `RearmPolicy{max_attempts=3,
  backoff_base}` is one policy instance, not the contract.

R4 [5]: A convergence combinator's public surface MUST live at the async
  edge (a `cherry-pit-app` or `cherry-pit-core` port trait); it MUST NOT
  appear on `pardosa::store`'s synchronous public facade (PGN-0010:R5) and
  MUST NOT appear in `pardosa-nats` (ring purity, PGN-0016:R6).

R5 [5]: The combinator's error type MUST be `#[non_exhaustive]`
  (CHE-0021), and every retry MUST remain gated on
  `ErrorCategory::Retryable` and idempotency-keyed per CHE-0046:R1/R3,
  inheriting rather than replacing that contract.

R6 [6]: Extraction of this combinator into a shared cherry-pit crate is
  deferred until a second cherry-pit consumer needs cross-run
  converge-after-fence; until that trigger fires, `converge_on_fence`
  remains gh-report-local and this ADR governs boundary compliance only,
  not a build obligation. This is the CHE-0037/CHE-0040 deferral-precedent
  shape: NOT a permanent refusal, a trigger-gated NOT YET.

## Consequences

+ becomes easier: a future port extraction has a pre-ratified contract to
  implement against instead of ad-hoc design; R1/R2/R4 foreclose the
  hand-roll shapes (in-append resync, sync-facade leakage, ring-purity
  violation) that would otherwise need re-litigating per callsite.
+ becomes easier: CHE-0088:R9's single-sink citation now rests on a
  substrate-level boundary ADR, not only a gh-report-local rule.
− becomes harder: future cross-run convergence work must justify R1-R5
  compliance explicitly rather than free-hand designing a resync shape.
risks/migration: ratifies boundary only; authorizes no code change, builds
  no port. Trigger to extract: a second concrete cherry-pit consumer
  needing cross-run converge-after-fence (CHE-0037/CHE-0040 analogue).
  Until then `converge_on_fence` stays gh-report-owned and CHE-0088 stays
  its citing ADR (see CHE-0088 scope-widen accompanying this ADR).
