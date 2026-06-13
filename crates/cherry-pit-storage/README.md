# cherry-pit-storage

Synchronous filesystem primitives — atomic writes, run-locks, and
content-addressable signatures — for cherry-pit consumers. Per CHE-0053:
absorbs `error`, `fs`, `lock`, and `signature` from the donor
`quics-memoization` crate.

**Status**: Implemented (v0.1). Public API surface is flat over private
modules (`error`, `fs`, `lock`, `signature`) per CHE-0053:R3 — see
`docs/adr/cherry/CHE-0053-cherry-pit-storage-design.md`
for the design and CHE-0053:R3 for the enumerated re-export set.
