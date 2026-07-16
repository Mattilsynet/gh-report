# CHE-0082. gh-report Collection Health Taxonomy

Date: 2026-06-17
Last-reviewed: 2026-07-16
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

R4 [5]: Treat public branch-protection 404 with no controls as genuine absence; it may remain a governance Fail.

R5 [5]: Treat a private or internal branch-protection 404 with no controls as genuine absence per R4, a governance Fail counted in the coverage denominator; reserve Unknown with a permission-suspected reason for authority failures (403, denied, rate-limited, or transient), never for the plain absent-control 404.

R6 [5]: Keep org-wide collection-health taxonomy counts in report-side aggregation, not on per-repository persisted payloads.

R7 [5]: Represent active credential limitations through the existing AuthMode, TokenTier, Capability, and unavailable-capabilities surfaces.

## Consequences

+ becomes easier: reports can distinguish weak governance from unreadable evidence and can name credential-driven blind spots.

− becomes harder: schema hashes move when new bounded event fields are appended, requiring re-scrape rather than mixed old/new event replay.

risks/migration: the first run without a GitHub App token reports private/internal branch-protection reads as capability-limited and classifies unreadable 404s as Unknown.
