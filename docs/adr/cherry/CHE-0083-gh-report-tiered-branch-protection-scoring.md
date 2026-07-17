# CHE-0083. gh-report Branch Protection Scoring

Date: 2026-06-17
Last-reviewed: 2026-07-17
Tier: B
Status: Accepted
Crates: gh-report

## Related

References: CHE-0082, CHE-0090, PGN-0013, COM-0019

## Context

gh-report branch-protection scoring used an AND-of-five gate: pull requests, one reviewer, status checks, admin enforcement, and no broad bypass. That made Scorecard bonus-level controls act as baseline gates. The 42-public-repo soak showed 28 genuine-absence repos and 14 weak ruleset repos; none had PR review enforcement. Force-push and deletion signals were absent from persisted evidence, so the minimal baseline could not be measured.

## Decision

Replace the branch-protection gate with a report-side Branch Protection Regime (BPR) model, per the BPR partition ratified in CHE-0090. Persist only raw per-repository signals; derive regimes and org-wide distributions in aggregation and rendering.

R1 [5]: Score BPR1 Unprotected (CHE-0090's below-baseline band) when branch protection is absent, unreadable as genuine absence, force pushes are not blocked, or deletion is not blocked.

R2 [5]: Score BPR2 IntegrityOnly when the default branch is protected and force-push plus deletion blocking are observed (CHE-0090's tier consistency map: BPR1<->BelowBaseline, BPR2<->Minimal — the BPR index does not track the old T-number by offset).

R3 [5]: Score BPR3 ReviewedWithBypass or BPR4 ReviewedGated (CHE-0090's split of the old accept bar) when BPR2 also requires pull requests with at least one approving review. Widened dashboard pass bar: branch-protection coverage counts a repository as PASS when its regime is BPR2 IntegrityOnly or higher — pass := regime in {BPR2, BPR3, BPR4, BPR5}; denominator remains all non-archived repos per adr-fmt-tm7ms.

R4 [5]: Treat required status checks, stale-review dismissal, and admin-equivalent enforcement as additive bonuses that raise a BPR3/BPR4 repository to BPR5 Hardened when present; absence never blocks BPR1 or BPR2.

R5 [5]: Treat broad or undocumented bypass actors as downgrades that cap the regime at BPR3 ReviewedWithBypass, not as an outright scoring failure.

R6 [5]: Append `force_push_blocked: Option<bool>` and `deletion_blocked: Option<bool>` to branch-protection details; `None` means unreadable or not observed and must not be fabricated as `Some(false)` for permission-suspected 404s.

R7 [5]: Keep the computed tier and org-wide tier distribution report-side only; do not persist them on per-repository event payloads.

## Consequences

+ becomes easier: branch-protection coverage now reflects a defendable accept bar while still showing weak-but-present baseline protection separately from genuine absence and unreadable evidence.

− becomes harder: report code must compute tier buckets from raw signals; old soak dumps without force-push or deletion fields can show only pre-H5 fixture shape, not honest tier ordering or final live distribution.

risks/migration: live before/after org numbers remain pending until a scoped App-token collection refreshes the raw H5 signals.
