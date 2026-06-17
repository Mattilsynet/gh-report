# CHE-0083. gh-report Tiered Branch Protection Scoring

Date: 2026-06-17
Last-reviewed: 2026-06-17
Tier: B
Status: Accepted
Crates: gh-report

## Related

References: CHE-0082, PGN-0013, COM-0019

## Context

gh-report branch-protection scoring used an AND-of-five gate: pull requests, one reviewer, status checks, admin enforcement, and no broad bypass. That made Scorecard bonus-level controls act as baseline gates. The 42-public-repo soak showed 28 genuine-absence repos and 14 weak ruleset repos; none had PR review enforcement. Force-push and deletion signals were absent from persisted evidence, so the minimal baseline could not be measured.

## Decision

Replace the branch-protection gate with a report-side tier model. Persist only raw per-repository signals; derive tiers and org-wide distributions in aggregation and rendering.

R1 [5]: Score T0 below-baseline when branch protection is absent, unreadable as genuine absence, force pushes are not blocked, or deletion is not blocked.

R2 [5]: Score T1 minimal when the default branch is protected and force-push plus deletion blocking are observed.

R3 [5]: Score T2 accept bar when T1 also requires pull requests with at least one approving review.

R4 [5]: Treat required status checks, stale-review dismissal, and admin-equivalent enforcement as additive bonuses that raise a T2 repository to T3+ when present; absence never blocks T1 or T2.

R5 [5]: Treat broad or undocumented bypass actors as downgrades that cap the bonus tier, not as an outright scoring failure.

R6 [5]: Append `force_push_blocked: Option<bool>` and `deletion_blocked: Option<bool>` to branch-protection details; `None` means unreadable or not observed and must not be fabricated as `Some(false)` for permission-suspected 404s.

R7 [5]: Keep the computed tier and org-wide tier distribution report-side only; do not persist them on per-repository event payloads.

## Consequences

+ becomes easier: branch-protection coverage now reflects a defendable accept bar while still showing weak-but-present baseline protection separately from genuine absence and unreadable evidence.

− becomes harder: report code must compute tier buckets from raw signals; old soak dumps without force-push or deletion fields can show only pre-H5 fixture shape, not honest tier ordering or final live distribution.

risks/migration: live before/after org numbers remain pending until a scoped App-token collection refreshes the raw H5 signals.
