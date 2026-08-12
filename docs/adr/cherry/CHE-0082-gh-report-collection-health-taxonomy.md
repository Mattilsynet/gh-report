# CHE-0082. gh-report Collection Health Taxonomy

Date: 2026-06-17
Last-reviewed: 2026-08-12
Tier: B
Status: Accepted
Crates: gh-report

## Related

References: CHE-0022, PGN-0013, COM-0019

## Context

gh-report previously collapsed unreadable GitHub evidence into the same governance outcome as genuine absence. Branch protection was the sharp failure: GitHub returns 404 both for absent public protection and for unreadable private or internal protection, while score aggregation counted both as Fail.

## Decision

Make collection health explicit. Per-repository raw failures carry typed reason and HTTP status where the collector knows them. Org-wide counts are derived in report projections and keyed by check kind plus reason, so rendering can separate posture from data quality.

R1 [5]: Persist per-repository collection-health facts only when they describe that repository's own check result.

R2 [5]: Append new gh-report event fields under CHE-0022 and PGN-0013; do not rename or reshape existing persisted fields.

R3 [5]: Store HTTP status as `Option<u16>` or an equivalent bounded enum, never as text or an HTTP library type.

R4 [5]: Treat a public branch-protection 404 with no controls as genuine absence (governance Fail, in the denominator) WHEN `Capability::PrivateBranchProtectionRead` is Available; otherwise (Unavailable, PermissionDenied, or not probed) treat it as Unknown with a permission-suspected reason. The legacy endpoint requires repo-admin regardless of visibility, so public 404s are equally uninformative (evidence: adr-fmt-bol6p).

R5 [5]: Treat a private or internal branch-protection 404 with no controls as genuine absence per R4 (governance Fail, in the denominator) WHEN `Capability::PrivateBranchProtectionRead` is Available; otherwise (Unavailable, PermissionDenied, or not probed) treat it as Unknown with a permission-suspected reason. Adds only the capability condition; does not revive inferring authority failure from 404 plus visibility alone, which stays forbidden (evidence: adr-fmt-bol6p).

R6 [5]: Keep org-wide collection-health taxonomy counts in report-side aggregation, not on per-repository persisted payloads.

R7 [5]: Represent active credential limitations through the existing AuthMode, TokenTier, Capability, and unavailable-capabilities surfaces.

## Consequences

+ becomes easier: reports can distinguish weak governance from unreadable evidence and can name credential-driven blind spots.

− becomes harder: schema hashes move when new bounded event fields are appended, requiring re-scrape rather than mixed old/new event replay.

− becomes harder: `Capability::PrivateBranchProtectionRead` now gates both public and private/internal 404 classification (R4, R5); the name predates that scope. Kept as-is — it is wire-format serialised and out of scope to rename here.

risks/migration: the first run without a GitHub App token reports public and private/internal branch-protection reads as capability-limited; per the R4/R5 amendment (2026-08-12, evidence adr-fmt-bol6p), a branch-protection 404 of any visibility is genuine absence only while `Capability::PrivateBranchProtectionRead` is Available, and is classified Unknown/permission-suspected otherwise.
