# cherry-pit-wq

Domain-agnostic concurrency and resource-pacing primitives for
cherry-pit consumers. Per CHE-0055 (supersedes CHE-0052): absorbs
`work_queue`, `worker_pool`, `budget`, and `rate_limit` from the donor
`quics-aggregate` crate. `pagination` is NOT in this crate — it relocated
to `gh-report::github::pagination` (CHE-0055:R7).

**Status**: Implemented (v0.1). Public API surface is the flat re-export
set per CHE-0055:R10 — see CHE-0055 in `docs/adr/cherry/`
for the design and CHE-0055:R10 for the enumerated re-export set.
