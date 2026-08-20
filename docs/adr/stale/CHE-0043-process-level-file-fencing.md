# CHE-0043. Process-Level File Fencing

Date: 2026-04-25
Last-reviewed: 2026-08-20 - refined - retirement narrative corrected: names what carried (run-lock invariant, now CHE-0053:R13) and fixes the PGN-0014 misroute
Tier: D
Status: Superseded by CHE-0100

## Retirement

Superseded-by: CHE-0100
Moved-to-stale: 2026-07-23
Reason: `MsgpackFileStore`, the store this ADR's fencing rules were
written against, is retired in full (CHE-0100).

Dropped: those layout-specific rules. Carried: the single-process TTL
run-lock invariant, whose mechanism still ships in `cherry-pit-storage`;
its live owner is now CHE-0053:R13. Narrative carry only; no lineage
edge fabricated (AFM-0030:R4, R6).

Correction 2026-08-20: this stub misrouted surviving fencing to
PGN-0014, which has no fencing rule; the cross-instance fence is
PGN-0016.
