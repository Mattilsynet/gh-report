# CHE-0043. Process-Level File Fencing

Date: 2026-04-25
Last-reviewed: 2026-07-23
Tier: D
Status: Superseded by CHE-0100

## Retirement

Superseded-by: CHE-0100
Moved-to-stale: 2026-07-23
Reason: `MsgpackFileStore`, the store this advisory file-lock fencing
mechanism protected, is retired in full (CHE-0100). Fencing for the
surviving pgno-backed store is PGN-0014's concern; no rule here
survives the store's deletion.
