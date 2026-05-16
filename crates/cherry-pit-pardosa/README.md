# cherry-pit-pardosa

Pardosa-backed [`EventStore`](https://docs.rs/cherry-pit-core/latest/cherry_pit_core/trait.EventStore.html)
adapter for cherry-pit.

## Status

v0.1 — minimum-viable surface targeted by SM-6 conformance. The CHE-0057
extension traits are implemented as follows:

| Trait                       | Status                                                                 |
| --------------------------- | ---------------------------------------------------------------------- |
| `PurgeableEventStore`       | Real — bridges to pardosa's `migrate_fiber(_, Purge)` + `create_reuse` |
| `HashChainedEventStore`     | **Rollout stub** per CHE-0060:R3 — removed when PAR-0021 lands         |
| `SingleWriterEventStore`    | Marker — substrate-level guarantee per PAR-0004:R1                     |

## Governing ADRs

CHE-0057 (extension-trait composition), CHE-0059 (`PurgeableEventStore`),
CHE-0060 (`HashChainedEventStore`), CHE-0061 (`SingleWriterEventStore`),
PAR-0001 (fiber state machine — `Purged → Defined`), PAR-0004
(single-writer fencing), PAR-0021 (per-stream BLAKE3 hash chain —
substrate-pending).

## License

Dual-licensed under `Apache-2.0 OR MIT`.
