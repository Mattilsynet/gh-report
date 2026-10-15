# CHE-0083. gh-report Branch Protection Scoring

Date: 2026-06-17
Last-reviewed: 2026-10-15
Tier: B
Status: Accepted
Crates: gh-report

## Related

References: CHE-0082, PGN-0013, COM-0019
Supersedes: CHE-0090

## Context

gh-report branch-protection scoring began as an AND-of-five gate (pull requests, one reviewer, status checks, admin enforcement, and no broad bypass), then a report-side Branch Protection Regime (BPR) ladder scored six bands over seven persisted signals. Both models anchored the baseline on pull-request review, which external-ruleset repositories can satisfy while leaving history rewrite unblocked. The 42-public-repo soak showed review-based bars are a poor history-protection proxy: they conflate a repository's code-review culture with the destructive-rewrite block baseline governance actually needs. This revision narrows branch protection to its history-protection core and removes the rest from the check.

## Decision

Replace the regime ladder with a history-protection-only closed sum, `BranchProtectionResult`, and give Branch Protection a place in the CoverageControl denominator drill-down.

R1 [5]: Model branch protection per repository as exactly one of `Protected` (both force pushes and branch deletion are blocked on the default branch), `Unprotected` carrying a `HistoryProtectionGap` (`Absent` | `NeitherBlocked` | `ForcePushAllowed` | `DeletionAllowed`), or `Unobservable` with a typed collection-health (`IndeterminateReason`) failure. A closed algebraic sum (CHE-0002); no other outcome exists.

R2 [5]: Score `Protected` as Pass/Met and `Unprotected` as Fail/Unmet. `Unobservable` is Excluded — never a compliance verdict (CHE-0082) — and under the CoverageControl model an unobservable repository leaves the Branch Protection denominator rather than swimming as a failure.

R3 [5]: Treat genuine absence of controls (`HistoryProtectionGap::Absent { confirmed_not_found }`, from an observed authority signal / admin-scoped 404) as `Unprotected` — a real, denominator-counted present finding — per CHE-0082:R4/R5.

R4 [5]: Persist only the history-protection raw signals (`force_push_blocked`, `deletion_blocked` when observed) and the resulting `HistoryProtectionGap`. The prior seven-signal `BranchProtectionDetails` and the report-side BPR ladder (CHE-0090) are removed from branch-protection evidence and scoring, along with the legacy `partial` outcome.

R5 [5]: Emit the change as a schema-major cutover (`EVIDENCE_SCHEMA_VERSION = 25.0`, fresh `-v25` streams) because the new computation makes prior projection evidence incomparable (COM-0027, OPERATIONS.md § Schema Versions → When to bump).

R6 [5]: Keep the org- and owner-level coverage rates consistent with the repo model: excluded controls leave the denominator at every level (heritage ruling; see `EVIDENCE_SCHEMA_VERSION = 16.0` in OPERATIONS.md § Version history).

## Consequences

+ becomes easier: baseline governance is one unambiguous fact — is destructive history rewrite blocked — instead of a six-band report-side cascade over privileges external rulesets do not reliably expose to the collecting token.

− becomes harder: repositories whose only defensive bar was PR review while history stayed mutable now read `Unprotected`; operators must re-baseline expectations from the `-v24` era.

risks/migration: schema-major cutover provisions fresh `-v25` streams and leaves `-v24` streams archived and untouched; old stores refuse and a documented re-scrape is required per OPERATIONS.md § Schema Versions.
