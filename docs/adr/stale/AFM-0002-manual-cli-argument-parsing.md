# AFM-0002. Manual CLI Argument Parsing Over Clap

Date: 2026-04-27
Last-reviewed: 2026-05-01
Tier: D
Status: Superseded by AFM-0013

## Retirement

Superseded-by: AFM-0013
Moved-to-stale: 2026-04-28
Reason: The argument surface grew beyond the five-flag reassessment
trigger in R4. Six modes with additional parameters now require
mutual exclusivity groups, help generation, and error formatting
that clap handles automatically. Migrated to clap derive API.
