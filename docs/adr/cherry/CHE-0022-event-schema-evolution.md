# CHE-0022. Event Schema Evolution Strategy

Date: 2026-04-25
Last-reviewed: 2026-05-18
Tier: B
Status: Accepted

## Related

References: CHE-0010, CHE-0009, CHE-0021, CHE-0065, CHE-0038, GEN-0015

## Context

Events are immutable facts persisted forever. Event enums grow as
domain models evolve. Under pardosa-genome (CHE-0065), the wire
format is fixed-layout (GEN-0035) and `#[serde(default)]` is
rejected at compile time by `#[derive(GenomeSafe)]` (GEN-0029).
Envelope-level forward compatibility is therefore realised via
GEN-0015's file-header version field with hard-cut discard-on-
mismatch semantics, paired with gh-report's operating policy of
re-scrape on schema bump (no production deployments, no data
migration).

## Decision

Additive-only event evolution:

R1 [5]: New enum variants are allowed and intentionally compile-breaking
  to force all apply implementations to handle them
R2 [5]: Removing or renaming persisted event variants is forbidden
R3 [5]: Adding, removing, or reshaping fields on existing variants
  is a schema-version bump per CHE-0065:R3 — the GEN-0015 file-header
  `format_version` is incremented and existing on-disk batches are
  discarded on read via `StoreError::SchemaVersionMismatch`
R4 [5]: event_type() strings are immutable once events exist in a log
R5 [5]: Do not use #[non_exhaustive] on domain event enums; exhaustive
  matching in apply is required

1. **New enum variants**: allowed. Adding a variant is intentionally
   a compile-breaking change — all `apply` implementations must be
   updated. This is correct: CHE-0009 (infallible apply) requires
   total event handling.
2. **Removing variants**: forbidden. Persisted events are immutable.
3. **Field shape changes on existing variants**: schema-version bump
   per CHE-0065:R3. The fixed-layout pardosa-genome format has no
   in-version additive evolution (GEN-0029, GEN-0035); evolution is
   realised by bumping GEN-0015's `format_version` and discarding
   pre-bump on-disk batches. gh-report's re-scrape policy
   (2026-05-17 user constraint) covers operational recovery.
4. **`event_type()` strings**: immutable once events exist in a log
   (per CHE-0010). Renaming breaks deserialization.
5. **`#[non_exhaustive]`**: NOT recommended on domain event enums.
   Unlike error types (CHE-0021), events require exhaustive matching
   in `apply` to maintain `state = f(events)`.
6. **Structural migration**: deferred to Pardosa (log-to-log rewrite
   with upcasters).

## Consequences

- Adding a variant forces compile-time updates to every `apply` — no silent ignoring.
- Field evolution requires a schema-version bump and discards pre-bump on-disk batches.
  This is a hard-cut posture explicitly accepted under gh-report's no-production /
  re-scrape policy; it does not extend to deployments lacking that policy.
- No runtime migration until Pardosa is built. Removing or renaming events requires a full Pardosa log migration.
- **Roll-forward only** — schema bumps and variant additions are both compile-breaking
  and on-disk-incompatible; rolling back code requires re-scrape. Silent data loss
  from ignoring unknown events is worse than a loud failure.
- **Golden-file serde regression** (CHE-0038) catches accidental format changes from dependency updates by comparing a deterministic envelope against a committed fixture byte-for-byte.
