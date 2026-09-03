# CHE-0082. gh-report Collection Health Taxonomy

Date: 2026-06-17
Last-reviewed: 2026-09-03 — refined — R2 corrected: pardosa structural genome hashing makes an event-field append a schema break, so R2 now cites CHE-0022:R3 instead of contradicting it (evidence: ghr-da3yy)
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

R2 [5]: `pardosa-derive`'s `build_field_hash_exprs` folds every event field's name and type into PGN-0003:R4 `SCHEMA_HASH`, so appending a gh-report event field is a reshape in effect. Such a change MUST be handled as the schema bump CHE-0022:R3 already mandates; mixed old/new replay MUST NOT be attempted. Enforcement is the substrate-computed hash gate, not review.

R3 [5]: Store HTTP status as `Option<u16>` or an equivalent bounded enum, never as text or an HTTP library type.

R4 [5]: Treat a public branch-protection 404 with no controls as genuine absence (governance Fail, denominator-counted) when an observed authority signal shows the caller had sufficient authority to read protection; otherwise treat it as Unknown, permission-suspected. The legacy endpoint requires elevated authority regardless of visibility, so an unauthorized caller's 404 carries no absence information (evidence: ghr-d1176f2a).

R5 [5]: Treat a private or internal branch-protection 404 with no controls as genuine absence per R4 (Fail, denominator-counted) under the same authority-signal condition; otherwise Unknown, permission-suspected, never plain absent-control. Inferring authority failure from a 404 plus private/internal visibility alone is forbidden: visibility is not an authority signal; this exception keys only on an observed authority signal, never on visibility.

R6 [5]: Keep org-wide collection-health taxonomy counts in report-side aggregation, not on per-repository persisted payloads.

R7 [5]: Represent active credential limitations through the existing AuthMode, TokenTier, Capability, and unavailable-capabilities surfaces.

## Consequences

+ becomes easier: reports can distinguish weak governance from unreadable evidence and can name credential-driven blind spots.

− becomes harder: schema hashes move when new bounded event fields are appended, requiring re-scrape rather than mixed old/new event replay.

risks/migration: the first run without a GitHub App token reports branch-protection reads as capability-limited; per the R4/R5 amendment (2026-08-19, evidence ghr-d1176f2a), a branch-protection 404 of any visibility is genuine absence only while an observed authority signal indicates sufficient authority to read protection, and is classified Unknown/permission-suspected otherwise. Per the R2 amendment (2026-09-03, evidence ghr-da3yy), appending a bounded event field moves `SCHEMA_HASH` and mandates refuse-and-re-scrape per CHE-0022:R3; the realised failure is the v18→v19 incident (`OPERATIONS.md:512`).
