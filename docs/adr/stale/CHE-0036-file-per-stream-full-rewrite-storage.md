# CHE-0036. File-Per-Stream Full-Rewrite Storage Model

Date: 2026-04-25
Last-reviewed: 2026-07-23
Tier: D
Status: Superseded by CHE-0100

## Retirement

Superseded-by: CHE-0100
Moved-to-stale: 2026-07-23
Reason: `MsgpackFileStore`, the store whose file-per-aggregate topology
this ADR governed, is retired in full (CHE-0100). No rule survives:
pgno's storage topology differs, and no other store adopts a
one-file-per-aggregate full-rewrite model.
