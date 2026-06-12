# CHE-0052. Cherry Pit Runtime Design

Date: 2026-05-09
Last-reviewed: 2026-05-09

Tier: B
Status: Superseded by CHE-0055

## Related

(no lineage edges — successor recorded in Status)

## Retirement

Superseded-by: CHE-0055
Moved-to-stale: 2026-05-13
Reason: CHE-0055 (`cherry-pit-wq`) ships the narrower work-queue/worker-pool
surface this ADR proposed under a more accurate crate name, with the
correlation propagation reach (R4/R6) explicitly deferred to v0.2 per
FOCUS.md §7+§8 ratification (surprise bead `adr-fmt-tm6m`). The full
five-module donor absorption envisioned here (budget, rate-limit, pagination
helpers in addition to work queue / worker pool) is collapsed under CHE-0055's
verbatim-port scope.
