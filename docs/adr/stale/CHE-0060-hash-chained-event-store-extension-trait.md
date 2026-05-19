# CHE-0060. HashChainedEventStore Extension Trait

Date: 2026-05-16
Last-reviewed: 2026-05-19
Tier: B
Status: Deprecated

## Related

(no lineage edges — this ADR is retired without successor)

## Retirement

Moved-to-stale: 2026-05-19
Reason: cherry-pit-pardosa, the sole in-tree substrate that hosted
the rollout-stub of this extension trait (R3's named carve-out), was
removed from `crates/`. PAR-0021's BLAKE3 precursor-chain and
`Dragline::frontier` capability remain defined at the pardosa-traits
layer; their future surface through cherry-pit (if needed) would be
re-introduced by a successor ADR rather than by reviving this one.
CHE-0064 retains its substrate-side Encode bound under its own
justification; the CHE-0060:R2 frontier-hash output shape is not
load-bearing for CHE-0064. Inbound citation at CHE-0064:11/:20/:65
points into stale per AFM-0022 (stale ADRs preserve identity for
historical references).
