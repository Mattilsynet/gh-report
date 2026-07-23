# CHE-0097. Projection Checkpoint Sequence Monotonicity

Date: 2026-07-23
Last-reviewed: 2026-07-23
Tier: B
Status: Accepted
Crates: cherry-pit-projection

## Related

References: CHE-0092, CHE-0046, CHE-0004, CHE-0043

## Context

`FileProjectionStore::persist` (`cherry-pit-projection/src/lib.rs`) writes a
snapshot and a `ProjectionCheckpoint` recording `last_sequence` on every
call, unconditionally, without reading the existing on-disk checkpoint
first. A caller that persists sequence 10 and later — via stale replay, a
racing rebuild, or a caller bug — persists sequence 3 silently downgrades
the checkpoint: a subsequent rebuild-from-checkpoint would re-apply
already-applied events, and any downstream consumer trusting the
checkpoint would treat stale progress as current.

This is distinct from CHE-0092, which governs read-model *content* merge
(status-priority anti-downgrade across `gh-report`'s team/org/repository
folds); its own fold sites are already guarded, not overclaiming. CHE-0097
governs the projection *store's* checkpoint-*sequence* bookkeeping in
`cherry-pit-projection` — a different crate, a different axis — hence a
standalone ADR rather than a CHE-0092 amendment.

## Decision

`FileProjectionStore::persist` enforces checkpoint-sequence monotonicity.

R1 [5]: `persist` reads the existing on-disk checkpoint (if any) for the target aggregate before writing. If the existing checkpoint's `last_sequence` is strictly greater than the sequence being persisted, the call returns `ProjectionError::CheckpointRegression` and performs no write (neither snapshot nor checkpoint file is touched).

R2 [5]: A persist call whose sequence equals or exceeds the existing checkpoint's `last_sequence` proceeds normally (equal-sequence re-persist, e.g. an idempotent retry, is not a regression).

R3 [5]: `ProjectionError::CheckpointRegression` classifies as `ErrorCategory::Terminal` (CHE-0046) — a sequence regression signals a caller-side ordering bug, not a transient condition; blind retry does not fix stale sequence input.

R4 [5]: This is a store-level guard, independent of and layered underneath CHE-0092's fold-content merge rule; a caller can still overwrite *content* for a fresh higher-sequence observation, but can never regress the *sequence marker* itself.

## Consequences

+ becomes easier: a stale or out-of-order persist call fails loudly at the
  store boundary instead of silently corrupting the checkpoint; rebuild-
  from-checkpoint can trust `last_sequence` as a true high-water mark.

- becomes harder: every `persist` call now pays one extra checkpoint read;
  callers that intentionally re-persist an old sequence (there are none
  today) must handle the new error variant.

risks/migration: additive read-before-write guard on an already-locked path
(the per-aggregate lock in `persist_inner` already serializes concurrent
callers); no schema or wire-format change. `ProjectionError` is
`#[non_exhaustive]`, so `CheckpointRegression` is additive to already-open
match arms.
