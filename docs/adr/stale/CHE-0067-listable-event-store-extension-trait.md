# CHE-0067. ListableEventStore Extension Trait

Date: 2026-05-20
Last-reviewed: 2026-05-20

Tier: B
Status: Superseded by CHE-0070

## Retirement

Superseded-by: CHE-0070
Moved-to-stale: 2026-06-10
Reason: CHE-0070 supersedes this ADR's R5 to permit the async
signature required by CHE-0018:R2 (synchronous-domain /
asynchronous-infrastructure boundary). CHE-0067 was the only
infrastructure-port method in `cherry-pit-core` with a sync I/O
signature; findings F11 (bd `adr-fmt-cq7vb.6`) surfaced the
reactor-stall hazard `MsgpackFileStore::list_aggregates` produces
when invoked from `async fn` callers via blocking `std::fs::read_dir`.
The supersession trigger is CHE-0067:R5 verbatim ("changing the
return shape requires a superseding ADR") + CHE-0057:R5 ("removal or
signature change requires superseding the extension's ADR"). R1–R4
carry forward unchanged into CHE-0070; R5 substantively changes;
CHE-0070 adds R6 governing the `spawn_blocking` + `JoinFailure`
mapping for substrates whose enumeration calls blocking syscalls.
