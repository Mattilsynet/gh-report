# CHE-0071. cherry-pit-pardosa Adapter

Date: 2026-06-10
Last-reviewed: 2026-06-11
Tier: B
Status: Superseded by CHE-0074

## Related

(no lineage edges — successor recorded in Status)

## Retirement

Moved-to-stale: 2026-06-11
Reason: P3-i replaces gh-report's `cherry-pit-pardosa` byte adapter with CHE-0074's native gh-report pardosa store port. gh-report now maps domain events into native GenomeSafe payloads and persists one pardosa fiber per repository domain key through the public resume-defined facade, so CHE-0071's opaque `EventEnvelope`-as-bytes adapter contract no longer governs gh-report.
