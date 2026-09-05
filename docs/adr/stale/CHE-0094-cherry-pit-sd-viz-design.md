# CHE-0094. cherry-pit-sd-viz Design

Date: 2026-07-18
Last-reviewed: 2026-09-05 — retired — subject crate deleted; moved to stale per AFM-0022:R1+R2
Tier: B
Status: Deprecated

## Retirement

Superseded-by: none (retired)
Moved-to-stale: 2026-09-05
Reason: This ADR designed `cherry-pit-sd-viz`, an unmounted standalone wasm
demo deleted from the workspace on 2026-09-05 with zero runtime consumers.
With its subject gone every rule here governs nothing, so it is retired rather
than superseded; no successor replaces it, following the AFM-0005 precedent for
an ADR whose subject ceased to exist. AFM-0030 does not govern — its R1 scopes
that charter to the pardosa domain. Deleting `## Decision` per AFM-0022:R2
discharges R8, which mandated the pardosa-dep tripwire; that gate was removed
too, since a `cargo tree` probe against a deleted package cannot fail and would
report green forever. CHE-0084:R9 was amended alongside; its R5 invariant stays
binding. Prior content is preserved in git history.
