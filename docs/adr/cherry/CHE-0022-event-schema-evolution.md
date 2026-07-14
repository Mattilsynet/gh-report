# CHE-0022. Event Schema Evolution Strategy

Date: 2026-04-25
Last-reviewed: 2026-06-14 — refined — narrowed R6 to per-entity payloads; designated snapshot events may carry aggregates under latest-event single-source semantics (mission:naz3i)
Tier: B
Status: Accepted

## Related

References: CHE-0010, CHE-0009, CHE-0021, CHE-0074, CHE-0038, PGN-0004, CHE-0048, CHE-0051

## Context

Events are immutable facts persisted forever. Event enums grow as
domain models evolve. Under the pardosa adapter (CHE-0071), the wire
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
  is a schema-version bump at the adapter boundary: legacy adapters bump
  GEN-0015 `format_version`, while gh-report's native pardosa port changes
  PGN-0003 `SCHEMA_HASH`; old-schema data is refused and re-scraped under
  PGN-0009.
R4 [5]: event_type() strings are immutable once events exist in a log
R5 [5]: Do not use #[non_exhaustive] on domain event enums; exhaustive
  matching in apply is required
R6 [5]: Per-entity/current-state event payloads MUST NOT carry computed
  aggregates derived from OTHER events. They carry only raw signals within one
  aggregate's scope; derived cross-entity state is replayed (CHE-0051:R5) and
  persists, if at all, only as a CHE-0048 projection checkpoint, avoiding the
  `baseline.msgpack` / δ.3c-ii / `63236ac` parallel-truth failure. Designated
  snapshot/summary events MAY carry own-scope aggregates under latest-event
  single-source semantics.

1. **New enum variants**: allowed. Adding a variant is intentionally
   a compile-breaking change — all `apply` implementations must be
   updated. This is correct: CHE-0009 (infallible apply) requires
   total event handling.
2. **Removing variants**: forbidden. Persisted events are immutable.
3. **Field shape changes on existing variants**: schema-version bump
   per adapter governance. The fixed-layout pardosa path has no
   in-version additive evolution (GEN-0029, GEN-0035); evolution is
   realised by bumping GEN-0015's `format_version`, or by changing the
   PGN-0003 `SCHEMA_HASH` for gh-report's native pardosa port, and
   discarding pre-bump on-disk batches. gh-report's re-scrape policy
   (2026-05-17 user constraint) covers operational recovery; PGN-0009
   governs the native-port refuse-then-rescrape posture.
4. **`event_type()` strings**: immutable once events exist in a log
   (per CHE-0010). Renaming breaks deserialization.
5. **`#[non_exhaustive]`**: NOT recommended on domain event enums.
   Unlike error types (CHE-0021), events require exhaustive matching
   in `apply` to maintain `state = f(events)`.
6. **Structural migration**: deferred to Pardosa (log-to-log rewrite
   with upcasters).
7. **No computed aggregates in per-entity payloads**: per-entity and
   current-state events carry raw signals within their own aggregate's
   scope; derived cross-entity views are reconstructed by replay
   (CHE-0051:R5) and live only there. Designated snapshot/summary
   events may carry aggregates only when the event is the sole authority
   under latest-event semantics and the same fields are not re-derived
   from another stream. δ.3c-ii (gh-report commit `63236ac`) retired
   `baseline.msgpack` and the sweep-level checkpoint precisely because
   they encoded such a parallel truth.

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
- **Derived cross-entity state from per-entity payloads lives only in projections** — CHE-0048 checkpoint topology, reconstructed via CHE-0051:R5 replay. A designated snapshot event may carry its own aggregate payload only under latest-event single-source semantics; a parallel truth in event payloads is the failure mode δ.3c-ii eliminated (gh-report commit `63236ac`).
