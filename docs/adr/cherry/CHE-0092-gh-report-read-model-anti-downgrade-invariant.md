# CHE-0092. gh-report Read-Model Monotonic-Merge (Anti-Downgrade) Invariant

Date: 2026-07-17
Last-reviewed: 2026-07-17
Tier: B
Status: Accepted
Crates: gh-report

## Related

References: CHE-0073, CHE-0089, CHE-0082, CHE-0075, CHE-0004

## Context

CHE-0089 is team-scoped ("Team ... per-team fiber"). This invariant must
apply uniformly across team, org, and repository folds. Amending CHE-0089 in
place would ratify a repo/org rule inside a team-scoped ADR — a scope
violation, own-scope discipline applied to the ADR itself — and would leave
CHE-0073 (which actually governs the repo/org folds) silent on the same
rule. AFM-0029:R2 amend-in-place is for changes within an ADR's existing
scope; this is a new cross-fold concern that exceeds CHE-0089's scope, so it
earns a new ADR.

**The honest departure this ADR must name.** CHE-0075:R2 states a read port
never decides; CHE-0004:R3 places business invariants on the write/command
side. The mechanism this ADR ratifies — a status-priority merge rule inside
a projection fold — is, read literally, a deliberate departure from that
canon. This ADR names that departure explicitly rather than hiding it:
CHE-0089:R2 deliberately de-aggregated team ("Team is NOT the repository
aggregate") — there is no ratified write-side seat for team, org, or
repository refresh. gh-report models current-state as capture+fold (CQRS
read projection), not command-sourcing. Under that model, a monotonic-merge
/ resolution policy over equally-valid observations — "N captures reduce to
one resolved view; a degraded observation does not supersede a known-good
one" — is a normal fold-resolution concern, not a smuggled write-side
invariant. This framing is legitimate only if stated honestly on the record,
which is what this ADR does; leaving the departure implicit would have been
a rationalization.

This directly fixes the org regression (org read-model apply site is
unguarded, matching the shape of a prod-observed `FencedConflict` exposure
identical to team's shape before it was guarded) and closes the repository
gap confirmed the same way. A rule only one fold obeys is the smell this
design avoids: a business rule that only one entity obeys is the problem,
not read-side placement per se.

## Decision

Ratify a single status-priority anti-downgrade rule, applied uniformly to
all three read-model fold sites.

R1 [5]: Only a genuine fresh `Complete` observation may overwrite an existing `Complete` entry, at all three fold sites: team (`crates/gh-report/src/projection.rs:161-170`, already guarded via `existing_is_complete`/`incoming_is_complete`), org (`apply_org_state`, `crates/gh-report/src/projection.rs:261-264`, currently unguarded — unconditional `self.org_state = Some(...)`), and repository (currently unguarded, same unconditional-apply shape). A degraded/transient/deleted-status observation is folded as a no-op skip when the resident entry is already `Complete`.

R2 [5]: The envelope `detached` flag remains the sole removal signal (CHE-0073:R7, CHE-0089:R4) — this rule does not add a second removal mechanism; it only governs what happens to non-detached upserts. Illustrative shape (team's existing guard, as the pattern to generalise):

```rust
let existing_is_complete = existing.is_some_and(|e| e.status == Complete);
let incoming_is_complete = incoming.status == Complete;
if !existing_is_complete || incoming_is_complete {
    upsert(incoming);
}
```

The same two-line gate is added at the org and repository apply sites, using each fold's own status/completeness concept (`OrgStateSnapshot`, repository evidence completeness) rather than reusing team's type.

R3 [5]: This invariant is honestly named as a deliberate departure from CHE-0075:R2 (a read port never decides) and CHE-0004:R3 (business invariants on the write/command side): it is a bounded projection merge/resolution policy over equally-valid observations, legitimate specifically because gh-report's team/org/repository state is capture+fold with no ratified write-side aggregate seat (CHE-0089:R2) — not a smuggled write-side invariant. This ADR is the record of that honest departure; it is not to be read as license for further read-side decision-making beyond this named, bounded rule.

R4 [5]: The rule is applied uniformly across all three folds, enforced by CHE-0082:R5's classification origin (never call transient failure genuine absence — this invariant is the fold-side enforcement of that classification). Grandfathering a team-only guard while org or repository silently regress is the smell this ADR closes; a business rule that only one entity obeys is not acceptable under this ADR.

## Consequences

+ becomes easier: org and repository read-model state gets the same downgrade protection team already has; a single named rule replaces three independently-reasoned-about fold sites.

- becomes harder: org and repository apply sites must each define and thread their own completeness concept (`OrgStateSnapshot`, repository evidence completeness) rather than sharing team's type; each site needs its own regression coverage for the guard.

risks/migration: additive read-side guard, no SCHEMA_HASH move, no envelope change. Rollout is per-fold: apply the org guard, apply the repository guard, add regression tests per fold. Deferred to the implement mission; no code edited by this ratification.
