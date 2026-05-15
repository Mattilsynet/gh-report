# CHE-0022. Event Schema Evolution Strategy

Date: 2026-04-25
Last-reviewed: 2026-04-25
Tier: B
Status: Accepted

## Related

References: CHE-0010, CHE-0009, CHE-0021, CHE-0031, CHE-0038

## Context

Events are immutable facts persisted forever. Event enums grow as
domain models evolve. Envelope-level forward compatibility is handled
by named MessagePack encoding with `#[serde(default)]` (CHE-0031).
Domain event evolution — adding/removing enum variants and struct
fields — has no framework-level support. Pardosa migration is planned
but unbuilt.

## Decision

Additive-only event evolution:

R1 [5]: New enum variants are allowed and intentionally compile-breaking
  to force all apply implementations to handle them
R2 [5]: Removing or renaming persisted event variants is forbidden
R3 [5]: New fields on existing variants must be Option<T> with
  #[serde(default)]
R4 [5]: event_type() strings are immutable once events exist in a log
R5 [5]: Do not use #[non_exhaustive] on domain event enums; exhaustive
  matching in apply is required

1. **New enum variants**: allowed. Adding a variant is intentionally
   a compile-breaking change — all `apply` implementations must be
   updated. This is correct: CHE-0009 (infallible apply) requires
   total event handling.
2. **Removing variants**: forbidden. Persisted events are immutable.
3. **New fields on existing variants**: must be `Option<T>` with
   `#[serde(default)]` so historical events deserialize correctly
   (consistent with CHE-0031's named encoding).
4. **`event_type()` strings**: immutable once events exist in a log
   (per CHE-0010). Renaming breaks deserialization.
5. **`#[non_exhaustive]`**: NOT recommended on domain event enums.
   Unlike error types (CHE-0021), events require exhaustive matching
   in `apply` to maintain `state = f(events)`.
6. **Structural migration**: deferred to Pardosa (log-to-log rewrite
   with upcasters).

## Consequences

- Adding a variant forces compile-time updates to every `apply` — no silent ignoring.
- Field evolution is constrained to optional additions — required fields break deserialization.
- No runtime migration until Pardosa is built. Removing or renaming events requires a full Pardosa log migration.
- **Roll-forward only** — rolling back code after writing new event variants makes affected aggregates unloadable until code rolls forward. Silent data loss from ignoring unknown events is worse than a loud failure.
- **Golden-file serde regression** (CHE-0038) catches accidental format changes from dependency updates by comparing a deterministic envelope against a committed fixture byte-for-byte.
