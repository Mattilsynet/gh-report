# ADR-026: No format auto-detection between bare messages and files

**Status:** Accepted

### Context

pardosa-genome defines two wire formats:

- **Bare messages:** Start with `format_version: u16 LE` at bytes 0–1.
- **Files:** Start with `"PGNO"` magic (4 ASCII bytes) at bytes 0–3, followed
  by `format_version: u16 LE` at bytes 4–5.

Auto-detection between these formats is trivially possible: check whether the
first 4 bytes are `"PGNO"`. If yes, file format; otherwise, bare message. This
would allow a single `parse(buf)` entry point.

A separate concern is compression auto-detection: bare messages include an
`algo` byte at offset 10 that enables transparent compression detection within
the bare message format. This IS implemented — `decode` auto-detects the
compression algorithm from the `algo` byte.

### Decision

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

### Consequences

- **Positive:** Zero ambiguity at the API level. No false-positive format
  detection possible.
- **Positive:** Separate error types for separate failure modes.
- **Negative:** Slight ergonomic friction — callers must know the format in
  advance. In practice, this is always known from context (file path vs.
  network buffer vs. IPC channel).
- **Negative:** Third-party tools that receive unknown pardosa-genome data
  must try both APIs or require out-of-band format indication.

### References

- ADR-008 (transport-agnostic core with companion crate separation)
- genome.md §Binary Format Specification: "No auto-detection. Consumers must
  use the correct API."
- `format.rs` — separate constants for bare headers (`BARE_HEADER_SIZE`) and
  file headers (`FILE_HEADER_SIZE`, `MAGIC`)
