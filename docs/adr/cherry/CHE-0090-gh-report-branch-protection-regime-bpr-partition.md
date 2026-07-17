# CHE-0090. gh-report Branch Protection Regime (BPR) Partition

Date: 2026-07-17
Last-reviewed: 2026-07-17
Tier: B
Status: Accepted
Crates: gh-report

## Related

References: CHE-0083, COM-0009

## Context

CHE-0083's scalar tier (BelowBaseline/Minimal/AcceptBar/Bonus) collapses two distinct AcceptBar shapes: reviewed-but-bypassable and reviewed-but-no-CI. A combination-based Branch Protection Regime (BPR) partition ratifies a report-side, non-persisted 6-band classification over the 7 already-persisted branch-protection signals, splitting AcceptBar into two bands while keeping every other band a 1:1 refinement of the existing tier.

## Decision

R1 [5]: Define BPR as a report-side derivation over the 7 persisted BranchProtectionDetails signals: BPR0 Unmeasured, BPR1 Unprotected, BPR2 IntegrityOnly, BPR3 ReviewedWithBypass, BPR4 ReviewedGated, BPR5 Hardened. Not a persisted field; inherits CHE-0083:R7 report-side-only.

R2 [5]: Classify via an ordered first-match-wins cascade: Excluded (status Unknown or reason in PermissionDenied/PermissionSuspected/Transient/RateLimited/Invalid) to BPR0; then the strength ladder BPR1..BPR5 per the qqilw cascade. NotFoundAbsent is not Excluded.

R3 [5]: The BPR-to-tier consistency map is binding: BPR0<->Excluded, BPR1<->BelowBaseline, BPR2<->Minimal, BPR3<->AcceptBar bypass-capped subcase, BPR4<->AcceptBar no-status-checks subcase, BPR5<->Bonus. BPR is a strict refinement of classify_branch_tier, not a relabel; only AcceptBar splits.

R4 [5]: The partition must be total and mutually exclusive: the cascade terminates in a final unconditional else (BPR5), so every input reaches exactly one band. Enforced by property tests per COM-0024:R2.

R5 [5]: admin_equivalent and the exact required_reviewers count are intra-band display refinements, not partition boundaries; only the reviewers>0 threshold and has_broad_bypass gate band membership.

## Consequences

+ becomes easier: drill-down views can distinguish bypass-capped from no-CI AcceptBar repos without a new persisted field or wire-format change.

− becomes harder: report code must implement and property-test a 6-arm cascade instead of consuming a single scalar tier.

risks/migration: none — pure report-side derivation over existing signals; no SCHEMA_HASH churn.
