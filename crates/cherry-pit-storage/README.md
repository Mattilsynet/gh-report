# cherry-pit-storage

Synchronous filesystem primitives — atomic writes, run-locks, and
content-addressable signatures — for cherry-pit consumers. Per CHE-0053:
absorbs `error`, `fs`, `lock`, and `signature` from the donor
`quics-memoization` crate.

**Status**: scaffolded (WU-6 / A6). Public API surface is populated by
WU-6 / A8 — see `docs/adr/cherry/CHE-0053-cherry-pit-storage-design.md`
for the target shape and CHE-0053:R3 for the enumerated re-export set.
