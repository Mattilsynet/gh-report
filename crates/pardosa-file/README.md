# pardosa-file

Payload-opaque `.pgno` file container: writer, reader, format constants, and
the `Syncable` durability seam.

Part of the [pardosa](https://github.com/acje/rescue-pardosa) workspace.

## Overview

`pardosa-file` is the on-disk substrate ring: it defines the byte-level
`.pgno` ("page-no", pronounced "page-know") format and exposes the
read / write surface that runtime crates (notably `pardosa`) compose into
journals. The format is fixed at the byte level and verified on
`Reader::open` — magic, `FORMAT_VERSION`, footer checksum, ascending
offsets, and per-message `xxh64` payload checksums (ADR-0006 "`.pgno` File
Format").

The substrate is payload-opaque: it carries `schema_hash: u128` in the
header so reads fail fast on schema mismatch (ADR-0005 "Encoding Contract"
+ ADR-0007 "Error Taxonomy"), but it never names the payload type. That
typing lives one ring up in `pardosa-schema` and `pardosa::store`.

Three writer flavours are exposed: `Writer<'_, W>` for single-pass
write-then-finish, `AppendWriter<'_, W>` for append-after-recovery, and
`PageClass` for tagging container roles. `Reader<R>` opens an existing
`.pgno`, validates the footer + offset index, and exposes
`MessageIter<'_, R>` for sequential reads.

`Syncable` is the durability seam: `Drop` is not a durability boundary
(ADR-0010 "Durability Levels"). The contract is explicit — callers fence
durability via `sync_data` before declaring bytes committed; `finish`
writes the footer (the only openable shape) but does not itself fsync.

The `zstd` feature adds optional payload compression. Native `.pgno`
structures (header, schema source, index, footer, checksums) stay
uncompressed regardless. The `test-support` feature exposes
fault-injection sinks under `test_support` for downstream test harnesses;
the surface is `#[doc(hidden)]` and excluded from semver guarantees
(ADR-0009 "Semver Policy" §judgement-primary).

## Quick start

```rust,no_run
use pardosa_file::{Writer, Reader};
use std::io::Cursor;

const SCHEMA_HASH: u128 = 0xDEAD_BEEF_CAFE_F00D_DEAD_BEEF_CAFE_F00D;

// Write a small .pgno into an in-memory buffer.
let mut sink = Cursor::new(Vec::new());
let mut writer = Writer::new(&mut sink, SCHEMA_HASH);
writer.write_message(b"hello")?;
writer.write_message(b"world")?;
writer.finish()?;

// Reopen and iterate.
let bytes = sink.into_inner();
let reader = Reader::open(Cursor::new(bytes))?;
assert_eq!(reader.message_count(), 2);
# Ok::<_, pardosa_file::FileError>(())
```

## Documentation

API docs: <https://docs.rs/pardosa-file>

## Architecture decisions

- ADR-0006 "`.pgno` File Format" — the byte-level container layout and
  the open-time validation contract.
- ADR-0010 "Durability Levels" — `Drop` is not a durability boundary;
  `sync_data` is the only fence.
- ADR-0005 "Encoding Contract" — `schema_hash: u128` carried in the header
  is derived from the payload type's structural shape; mismatch is fail-fast.

The full ADR set lives under [`docs/adr/`](../../docs/adr/).

## License

Licensed under either of

- Apache License, Version 2.0
- MIT License

at your option.
