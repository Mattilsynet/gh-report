# CHE-0093. gh-report Acknowledged Owner Anomaly (CODEOWNERS Governance Visibility)

Date: 2026-07-17
Last-reviewed: 2026-07-17
Tier: B
Status: Accepted
Crates: gh-report

## Related

References: CHE-0082, CHE-0089, CHE-0091

## Context

No existing ADR classifies CODEOWNERS owners or ratifies how a non-team
owner reference surfaces as governance signal — a genuine corpus gap. Prior
design recommended a one-time purge of stale `team_slug="*"` rows as an
out-of-scope operational follow-up. The user overrode that recommendation:
"would it be better to encode `*` team as an acknowledged team anomaly. we
see it, but it is not a configured team." The intent is subtractive-to-
additive: instead of deleting evidence (purge) or silently degrading (drop
at derivation without a trace), turn the smell into an explicit, surfaced
governance state.

**Acknowledged owner anomaly** — a CODEOWNERS-derived owner that gh-report
observes in real config but which is structurally not a resolvable GitHub
team. "Acknowledged" is load-bearing: the owner is neither hidden nor
deleted; it is recorded and surfaced as a known, named governance finding.

This is one concept with two sub-kinds, distinguished by where in the
pipeline the non-team-ness is discovered and by whether a fiber is ever
created:

| Sub-kind | What it is | Detected at | Current classification | Reaches a team fiber? |
|---|---|---|---|---|
| WildcardOwner | A glob-shaped owner token, e.g. `@org/*` — a catch-all/default-owner construct whose slug portion is a glob metacharacter. A bare `"*"` cannot be an owner (owner tokens must start with `@`, `codeowners_parser.rs:64`); the anomaly always arrives as an `@`-prefixed glob owner. | Owner classification time — `team_slug_from_canonical_owner` returns `None` on glob metachars; the owner is `OwnerType::AmbiguousTeamShaped`. | `AmbiguousTeamShaped` | No — filtered pre-API by commit `48ea545`; no live path derives a fiber. |
| GhostTeam | A plausible, well-formed slug that references a GitHub team that does not exist (HTTP 404), e.g. `virtual-environments-owners`. | Roster fetch time — `failure_status()` maps 404 to `TeamRosterStatus::Deleted`. | `OwnerType::Team` at classification, then `TeamRosterStatus::Deleted` at fetch. | Yes — it derives a fiber and the fiber genuinely detaches on confirmed 404. |

They are modelled together because the governance surface must present both
under one honest heading, while the lifecycle treatment differs precisely
because one derives a fiber and the other does not.

## Decision

Ratify acknowledged owner anomaly as gh-report ubiquitous language: a
governance annotation over an existing owner classification, not a new
`OwnerType` or `TeamRosterStatus` variant, surfaced without persisting a new
schema fact and without purging existing stale fibers.

R1 [5]: Name **acknowledged owner anomaly** as gh-report ubiquitous language, a governance annotation over an existing owner, with two sub-kinds `WildcardOwner` and `GhostTeam`. Illustrative shape (report/view-model scope; own-scope only):

```rust
enum OwnerAnomalyKind {
    WildcardOwner,
    GhostTeam,
}

struct AcknowledgedOwnerAnomaly {
    owner: CanonicalOwner,
    kind: OwnerAnomalyKind,
    referencing_repos: Vec<RepoName>,
}
```

This type is derived at the report projection, not persisted as a new event field (R3 — no SCHEMA_HASH move).

R2 [5]: The anomaly is a governance annotation computed at the report projection boundary from `(OwnerType, TeamRosterStatus, codeowners-origin)` — not a new `OwnerType` variant (`WildcardOwner` is already `AmbiguousTeamShaped`; adding an `Anomaly` variant would duplicate an existing distinction and force a breaking exhaustive-match change) and not a new `TeamRosterStatus` variant (`GhostTeam` is already `TeamRosterStatus::Deleted`; a new status variant would leak a governance framing into the roster read model, which CHE-0089 scopes to freshness/timing, not classification). This is an own-scope derived reading (CHE-0022:R6): no cross-entity computed aggregate.

R3 [5]: No persisted schema change. The anomaly is a report-side derived reading, not a durable event fact — computed from data already captured (`OwnerType`, `TeamRosterStatus`, CODEOWNERS origin) at projection time. Persisting a new durable anomaly field would move SCHEMA_HASH (CHE-0082:R2 / CHE-0022); this design deliberately declines that. A future mission wanting a durable anomaly fact (e.g. for alerting on first observation) needs a separate ADR that owns the schema move.

R4 [5]: Lifecycle treatment recognises the anomaly at classification without churning a fiber. WildcardOwner is already recognised at derivation — no team slug, no fiber, no GitHub call, no `TeamRosterStatus`; under this ADR that filtering is re-framed from a silent drop into an acknowledged governance finding retained in the report projection's anomaly view. GhostTeam does derive a fiber and does legitimately detach on confirmed 404 (genuine-absence detach, governed by CHE-0091); this ADR does not change that lifecycle, it only re-labels the surface so a 404-team is shown as an acknowledged anomaly distinct from a team that was once real and later deleted.

R5 [5]: Surface acknowledged owner anomalies on the report as a distinct governance/config-health view, in the same table style as other tables in this repo. The existing "Deleted" page (`DeletedViewModel`, `view_model.rs`, `html.rs:1145-1199`) is the attach point — it already renders the `GhostTeam` case under a "Deleted Teams" heading that conflates deleted with never-existed. Reframe that page to carry an "Acknowledged owner anomalies" section splitting the two sub-kinds: WildcardOwner rows (owners like `@org/*` — "seen in CODEOWNERS, is a wildcard/catch-all construct, not a configured team," with no existing surface today) and GhostTeam rows (owners like `virtual-environments-owners` — "referenced in CODEOWNERS, resolves to no GitHub team," migrating from the "Deleted Teams" framing to the anomaly framing). Reuse `build_owner_repo_map` to list referencing repos per anomalous owner. No new dashboard page is introduced.

R6 [5]: Given no purge, the steady state for existing stale `"*"` fibers is: no fresh `"*"` fibers are created (commit `48ea545` guarantees this at derivation, closed and non-growing population); existing stale `"*"` fibers self-detach harmlessly via the existing "team no longer owns any repository" detach path — no rescue is warranted, since there is no live team to rescue to (CHE-0091 governs rescue eligibility; a `WildcardOwner` fiber that detaches is not a rescue candidate); the anomaly view reads them as acknowledged, surfacing under WildcardOwner in the governance view rather than being deleted or silently dropped.

## Consequences

+ becomes easier: stale `*`-owner and ghost-team signal becomes an explicit, named governance finding instead of a silent drop or a data-deleting purge; one honest heading covers both sub-kinds without a new dashboard page.

- becomes harder: the "Deleted" page's heading and section structure must be reframed to distinguish never-existed from was-deleted; report code must derive the anomaly annotation from existing classification signals at projection time.

risks/migration: none — pure report-side derivation over existing signals (`OwnerType`, `TeamRosterStatus`, CODEOWNERS origin); no SCHEMA_HASH churn, no purge migration. Deferred to the implement mission; no code edited by this ratification.
