# GEN-0026. No Format Auto-Detection — Bare vs File

Date: 2026-04-25
Last-reviewed: 2026-04-28
Tier: D
Status: Accepted

## Related

References: GEN-0008

## Context

pardosa-genome defines two wire formats: bare messages (starting with `format_version: u16 LE`) and files (starting with `"PGNO"` magic). Auto-detection introduces ambiguity, forces a combined error type, and merges stateless and stateful APIs. Compression auto-detection within bare messages via the `algo` byte IS implemented and unaffected.

## Decision

**No auto-detection between bare and file formats.** Consumers must use the
correct API:

- `encode` / `decode` / `decode_with_options` for bare messages.
- `Writer` / `Reader` for files.

Compression auto-detection within bare messages IS supported via the `algo`
byte — this is not affected by this decision.

### Rationale

1. **Unambiguous API usage.** A bare message whose first two bytes happen to
   be `0x50 0x47` ("PG" in ASCII — a valid `format_version` value of 18256)
   could theoretically confuse an auto-detector. Explicit API selection
   eliminates this ambiguity entirely.
2. **Different error types.** Bare decode returns `DeError`; file parsing
   returns `FileError`. A unified entry point would need a combined error
   type, complicating error handling.
3. **Different lifetime semantics.** `Reader` holds state (parsed index,
   message count). `decode` is a stateless function. Merging these into one
   API would force allocation for single-message bare decodes.
4. **Explicit is better than implicit.** Callers know whether they're reading
   a file or a network message. Forcing the correct API reflects this
   knowledge in the type system.

R1 [9]: No auto-detection between bare and file formats — consumers
  must use the correct API
R2 [9]: Compression auto-detection within bare messages is supported
  via the algo byte
R3 [9]: Separate error types DeError and FileError are used for bare
  and file formats respectively

## Consequences

- Zero ambiguity at the API level. No false-positive format detection.
- Separate error types for separate failure modes.
- Slight ergonomic friction — callers must know the format. In practice always known from context.
- Third-party tools receiving unknown data must try both APIs or require out-of-band format indication.
