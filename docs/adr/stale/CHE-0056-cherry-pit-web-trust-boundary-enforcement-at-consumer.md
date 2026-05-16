# CHE-0056. Cherry Pit Web Trust-Boundary Enforcement at Consumer

Date: 2026-05-14
Last-reviewed: 2026-05-14

Tier: B
Status: Superseded by CHE-0062

## Retirement

Superseded-by: CHE-0062
Moved-to-stale: 2026-05-16
Reason: CHE-0062 reverses the R1 prohibitions on `DefaultBodyLimit`,
`ConcurrencyLimitLayer`, and WS-semaphore attachment by the library,
and authorises a library-owned `LayerLimits` value type rather than
the consumer-typed `&ValidatedConfig` parameter CHE-0056's Consequences
forbade. The supersession trigger is CHE-0056:R5 verbatim: Phase 2 v2
Track 4 (mission package `phase2-v2-track-4`, bead `adr-fmt-roqu`,
epic `adr-fmt-ysaa`) consolidates gh-report onto cherry-pit-web,
making the consumer-side duplication load-bearing in advance of the
"third consumer" wording — the first consumer's plumbing of two
Extension layers plus three sizing knobs is what R5 names. Track 4.1
inventory (bead `adr-fmt-czu1`) tagged the three layers as (a)
reusable upstream; oracle summary `adr-fmt-u3nf` confirmed CHE-0056
as the binding ADR blocking that push-up.
