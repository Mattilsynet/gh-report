# CHE-0090. gh-report Branch Protection Regime (BPR) Partition

Date: 2026-07-17
Last-reviewed: 2026-10-15
Tier: B
Status: Superseded by CHE-0083

## Retirement

Moved-to-stale: 2026-10-15
Reason: CHE-0083 (2026-10-15 revision) narrows branch-protection governance to history protection only. The persisted seven-signal `BranchProtectionDetails` and the report-side BPR ladder are removed from branch-protection evidence and scoring; `BranchProtectionResult` is a closed history-protection sum (`Protected` / `Unprotected` + `HistoryProtectionGap` / `Unobservable`) and its `Unobservable` arm carries the `IndeterminateReason` Excluded set this ADR's R2 first defined. No rule in this ADR survives the removal. The prior decision body remains in git history.
