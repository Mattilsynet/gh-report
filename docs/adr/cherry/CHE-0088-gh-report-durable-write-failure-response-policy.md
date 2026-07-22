# CHE-0088. gh-report Durable-Write-Failure Response Policy

Date: 2026-07-15
Last-reviewed: 2026-07-22
Tier: B
Status: Accepted
Crates: gh-report (CHE-0095 governs the substrate convergence-combinator
boundary this policy's R9 cites; this ADR's content — fixed backoff,
`WritePolicyCategory` taxonomy — remains gh-report tuning, not substrate
truth)

## Related

References: CHE-0024, CHE-0046, PGN-0016, COM-0025, CHE-0095 | Supersedes: none

## Context

Five production call sites (`record_repo` ×2, `record_org`, `mark_repo_deleted`, `remove_repo`) handle `PersistenceError` inconsistently — three swallow-and-log, two propagate-fatal — with no ratified rule deciding which response fits which failure. `native_store_persistence` (state.rs:818-833) also collapses four of six `StoreError` variants into one `LoadFailed` bucket, destroying the transient/structural/unrecoverable distinction a policy needs. COM-0025:R1/R6 discharge the port-typing and adapter-recovery obligations but do not prescribe caller response (oracle adr-fmt-fn453, Q1); PGN-0016:R2 pins conflict-class response but leaves the NATS-unavailable/transient class undelegated to gh-report (Q2). This ADR fills that hole.

## Decision

gh-report classifies every durable-write failure into a closed, gh-report-owned category enum at one conversion chokepoint, and dispatches each category to exactly one of three responses — `fatal`, `bounded-retry`, `HTTP-non-2xx` — via an exhaustive match with no wildcard arm.

R1 [5]: `native_store_persistence` (state.rs:818-833) widens to preserve `StoreError::BackendInfrastructure`/`Infrastructure` as a distinct `PersistenceError` variant (transient), `DivergedFiber` as a distinct variant (structural), and `Poisoned` as a distinct variant (unrecoverable), alongside the existing `FencedConflict`/`TornWriteRecovery` mappings; no `StoreError` variant may collapse into `LoadFailed` going forward.

R2 [5]: gh-report defines its own closed `WritePolicyCategory` enum, NOT `#[non_exhaustive]`, converting the `#[non_exhaustive]` `PersistenceError` into this closed set at exactly one chokepoint so every downstream policy dispatch is a total, compile-time-checked match.

R3 [5]: `WritePolicyCategory::Conflict` (from `FencedConflict`/`ConcurrencyConflict`) maps to `fatal` unconditionally, inheriting PGN-0016:R2's abort-the-run rule; no caller may catch and swallow a `Conflict` category, and any per-callsite code path that would do so is forbidden by this rule.

R4 [5]: `WritePolicyCategory::Transient` (from `BackendInfrastructure`/`Infrastructure`, i.e. NATS-unavailable) maps to `bounded-retry` at every call site — startup, sweep/reconcile, delivery-loop, and webhook alike — resolving the prior startup-fatal-vs-loop-swallow asymmetry into one unified policy for the same underlying failure class.

R5 [5]: `WritePolicyCategory::Structural` (`DivergedFiber`) and `WritePolicyCategory::Unrecoverable` (`Poisoned`, `TornWriteRecovery`) map to `fatal`; these represent invariant violations or corrupted process state that must abort rather than retry or silently continue.

R6 [5]: The webhook `remove_repo` persist-failure call site maps its resulting category to `HTTP-non-2xx` specifically at the HTTP-response layer — never a silent 200 — so GitHub redelivers the webhook; redelivery safety rests on the OCC fence and R3's unconditional Conflict→fatal mapping (PGN-0016:R2/R11), not on the bounded `Nats-Msg-Id` dedup window (PGN-0016:R5): that window only optimizes by suppressing exact retries while it is open and `EXPIRES` after 2 minutes, so it cannot serve as the idempotency backstop for redelivery that outlives it.

R7 [5]: The response-dispatch match over `WritePolicyCategory` has no catch-all wildcard arm; adding a new category to the enum without adding its response arm fails to compile, making per-callsite silent-swallow un-writable rather than merely discouraged.

R8 [5]: The three-value response vocabulary (`fatal`, `bounded-retry`, `HTTP-non-2xx`) is closed for this ADR; dead-letter is explicitly excluded as a response option here and tracked separately (see Consequences).

R9 [5]: A durable write that observes `WritePolicyCategory::Conflict` (R3) MUST converge via the single sanctioned resync+bounded-retry sink (`converge_on_fence`, `crates/gh-report/src/app/daemon.rs`) — no per-call-site hand-rolled re-arm. This extends R3's no-swallow rule and R7's no-wildcard-dispatch guarantee from the enum-dispatch chokepoint to the converge-sink chokepoint: both the collection loop and team-refresh route the same `FencedConflict` tick failure through the identical sink, so a fix to the re-arm/resync policy lands once, not per call site. Consumer-side converge is the PGN-0016:R2-sanctioned cross-run re-establish-ownership path, never the R10-forbidden in-append resync (amended 2026-07-22, adr-fmt-3jptm; PGN-0016 remains the authoritative boundary — see References). CHE-0095 governs the substrate-level boundary this sink's shape must obey if ever lifted out of gh-report; until CHE-0095's deferral trigger fires, `converge_on_fence` stays the gh-report-owned instance and this rule its citing policy.

R10 [5]: CI enforces R9 with a build-time tripwire (`fence-converge-tripwire`, `.github/workflows/ci-reusable.yml`, mirroring `projection-lock-tripwire` ← CHE-0048:R2 and the async-trait tripwire ← CHE-0025:R1/CHE-0029:R4): every production loop-body function in `daemon.rs` that detects a `FencedConflict` tick failure must, in the same function body, call the sanctioned sink's driver wrapper (`rearm_fenced_run`/`rearm_fenced_team_refresh_tick`); absence of that call fails the build (amended 2026-07-22). This is the mechanism that would have caught the original bug — team-refresh warned-and-returned on `FencedConflict` without re-arming.

## Consequences

+ becomes easier: every durable-write caller has one ratified response per failure category, closing the swallow-vs-fatal drift and the jxma5/5dhb1 asymmetries by rule, not convention.

− becomes harder: `native_store_persistence` grows two new variants beyond the current six; every call-site match must route through the new chokepoint.

risks/migration: mission-03 implements `WriteOutcome`/`WritePolicyCategory` and rewires the five call sites; this ADR ratifies content only. Dead-letter wiring (CHE-0024:R5 schema) is deferred, tracked in bd bead adr-fmt-4uxio — not a response category here.
