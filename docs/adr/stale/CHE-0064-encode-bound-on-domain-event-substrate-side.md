# CHE-0064. Encode Bound on DomainEvent for Substrate-Side Hash Chaining

Date: 2026-05-18
Last-reviewed: 2026-06-10
Tier: B
Status: Superseded by CHE-0071

## Related

(no lineage edges — successor recorded in Status)

## Retirement

Moved-to-stale: 2026-06-10
Reason: M1 rejected the native substrate-side encoding path. CHE-0071 replaces it with a `cherry-pit-pardosa` adapter that stores opaque serde-encoded `EventEnvelope` bytes inside a `GenomeSafe` wrapper and reconstructs logical streams through the public pardosa fold seam.
