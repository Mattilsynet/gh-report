# CHE-0091. gh-report Detach/Rescue Event-Content Contract

Date: 2026-07-17
Last-reviewed: 2026-07-17
Tier: B
Status: Accepted
Crates: gh-report

## Related

References: CHE-0073, CHE-0089, CHE-0074, PGN-0002, PGN-0014

## Context

CHE-0073 R2/R7 established the detached-flag-sole-removal-signal pattern and
per-entity current-state capture. CHE-0089 R1 defines `TeamStateCaptured`.
Neither ADR — nor PGN-0002, PGN-0014, CHE-0074 R6 — specifies what the
`DomainEvent` body of a detach or rescue event should contain. Two existing
call sites already disagree: `crates/gh-report/src/app/state.rs:1498`
(`remove_repo`) nulls the `RepositoryEvidence` field as a soft tombstone by
caller convention, not a distinct type; `crates/gh-report/src/app/state.rs:1452`
(`detach_team`) clones the full, non-nulled `TeamStateCaptured` snapshot into
the detach event. The external Fiber-semantics spec is deliberately silent on
payload content — payload shape is out of scope at the substrate level, so
this is green-field design work, not a spec-conformance gap.

PGN owns the envelope and lifecycle-flag layer, exhaustively: PGN-0002
ratifies `event_id / fiber_id / precursor / precursor_hash / detached: bool /
domain_event: T` — "nothing else." The substrate cannot own `DomainEvent`
body content; `T` is opaque to it by design. No new PGN ADR is warranted; CHE
owns `DomainEvent` payload semantics for the gh-report consumer, which is
what this ADR ratifies.

## Decision

Ratify a minimal-tombstone content contract for detach and rescue event
bodies, name rescue as a first-class app-facade verb, and hold the payload
rule fold-neutral.

R1 [5]: A detach event's `DomainEvent` body MUST carry a minimal tombstone, not a full current-state clone. `remove_repo`'s null-body pattern (`state.rs:1498`) is the target shape; `detach_team`'s full-snapshot clone (`state.rs:1452`) is non-conforming under this rule and must change to match the repo pattern (reconciliation target for a later implement mission; no code edits here). The fold does not read the detach event's body at all (it reads only the envelope `detached` flag, CHE-0073:R7), so this rule is fold-neutral for the read model; it matters for CHE-0074:R6's external journal consumer, which does see raw event bodies, and for which a full-snapshot body on a detach event is misleading.

R2 [5]: `reason`, `actor`, `detached_at` are OPTIONAL fields on the CHE `DomainEvent` body — never on the PGN envelope, since PGN-0002's "nothing else" clause forbids that. They are optional-but-recorded-when-known: mandatory fields would force call sites without this information (e.g. an automated tick-driven detach with no human actor) to fabricate values. Illustrative shape (own-scope only, per CHE-0022:R6 — no cross-entity computed aggregates):

```rust
struct DetachTombstone {
    detached_at: DateTime<Utc>,
    reason: Option<DetachReason>,
    actor: Option<String>,
}

enum DetachReason {
    NoLongerExistsUpstream,
    NoLongerOwnsAnyRepository,
    OperatorRequested,
}
```

R3 [5]: A named app-facade verb (e.g. `rescue_team(domain_key)`) wraps the already-ratified PGN-0014 `rescue_detached` entry point (validated-`FiberId`, no dragline bypass). Today rescue happens only as an implicit side effect of `record()` on a detached key; this design names it as a first-class, discoverable action without changing the underlying mechanism — it must still clear removal via the envelope `detached` flag, not a competing domain "un-tombstone" marker. Rescue preserves history (append-only) unless a prune migration has already run against that fiber, in which case history is lost.

R4 [5]: detach-on-already-detached is a no-op (repeated detach is safe; the envelope flag is already true). rescue-on-never-detached is rejected/no-op (there is no detached state to rescue from; rescue is only a defined transition from Detached/Locked, not from Defined). rescue-after-detach ordering is enforced via the existing precursor chain (PGN-0002 `precursor: Option<...>` / `precursor_hash`) — no new ordering mechanism is introduced.

R5 [5]: The tombstone body carries no cross-entity computed aggregates (CHE-0022:R6 own-scope) — only own-scope raw signals plus the optional own-scope audit fields in R2. If a dedicated tombstone variant is introduced, the domain event enum keeps no `#[non_exhaustive]` on it (CHE-0022:R5) — exhaustive `match` required at all call sites.

## Consequences

+ becomes easier: journal consumers reading raw event bodies see one consistent minimal-tombstone shape instead of two disagreeing conventions; rescue becomes a discoverable, named action instead of an implicit side effect.

- becomes harder: `detach_team` must be reconciled to the minimal-tombstone shape (tracked as implement-mission work); call sites that want audit context must thread `reason`/`actor`/`detached_at` through explicitly.

risks/migration: additive on the DomainEvent body only, no envelope change, no SCHEMA_HASH move for the fold (fold reads only the envelope `detached` flag). Reconciling `detach_team` (`state.rs:1452`) to match `remove_repo` (`state.rs:1498`) is deferred to the implement mission.
