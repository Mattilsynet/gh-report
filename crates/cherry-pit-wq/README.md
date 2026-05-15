# cherry-pit-wq

Domain-agnostic concurrency and resource-pacing primitives for
cherry-pit consumers. Per CHE-0052: absorbs `work_queue`, `worker_pool`,
`budget`, `rate_limit`, and `pagination` from the donor `quics-aggregate`
crate.

**Status**: scaffolded (WU-6 / A5). Public API surface is populated by
WU-6 / A7 — see CHE-0052 in `docs/adr/cherry/`
for the target shape and CHE-0052:R3 for the enumerated re-export set.
