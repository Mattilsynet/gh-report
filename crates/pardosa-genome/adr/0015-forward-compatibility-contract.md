# ADR-015: Forward compatibility contract — version and magic field positions frozen

**Status:** Accepted

### Context

Binary formats evolve. When a parser encounters data from a future format version,
it must be able to detect the version mismatch and reject gracefully — before
interpreting any subsequent fields whose layout may have changed.

This requires that the version identification bytes occupy a fixed, permanent
position in the format. If the version field itself moves between versions, no
parser can reliably read it.

### Decision

The following byte positions are **frozen** — they will never change across any
future format version:

**File format:**

| Bytes | Field | Value (v1) | Contract |
|-------|-------|-----------|----------|
| 0–3 | `magic` | ASCII `"PGNO"` (0x50 0x47 0x4E 0x4F) | Position, size, encoding frozen |
| 4–5 | `format_version` | `1` (u16 LE) | Position, size, encoding frozen |

**Bare message format:**

| Bytes | Field | Value (v1) | Contract |
|-------|-------|-----------|----------|
| 0–1 | `format_version` | `1` (u16 LE) | Position, size, encoding frozen |

**Reader behavior:** Readers must read these fields first and reject unknown
versions with `DeError::VersionMismatch` (bare) or `FileError::UnsupportedVersion`
(file) before interpreting any subsequent fields.

**Writer behavior:** Writers must always emit the current format version. No
backward-version writing mode.

The magic bytes `"PGNO"` serve as a file-type sentinel — they distinguish genome
files from other binary formats and enable fast rejection of non-genome input.
The bare message format has no magic bytes (it starts directly with
`format_version`) because bare messages are used in contexts where the transport
layer already identifies the content type.

### Consequences

- **Positive:** Any future parser can detect and cleanly reject data from
  unknown format versions, regardless of how the rest of the format changes.
- **Positive:** Simple version negotiation — readers check 2 bytes (bare) or
  6 bytes (file) and halt on mismatch.
- **Negative:** The `u16` version field limits the format to 65,535 versions.
  Sufficient for the expected evolution rate.
- **Negative:** Cannot repurpose bytes 0–1 (bare) or 0–5 (file) for any other
  use in future versions.

### References

- `format.rs` — `MAGIC`, `FORMAT_VERSION`, `HEADER_MAGIC_OFFSET`,
  `HEADER_VERSION_OFFSET`
- `error.rs` — `DeError::VersionMismatch`, `FileError::UnsupportedVersion`
- genome.md §Binary Format (forward compatibility contract)
