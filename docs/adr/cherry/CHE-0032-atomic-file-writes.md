# CHE-0032. Atomic File Writes via Temp-File and Rename

Date: 2026-04-25
Last-reviewed: 2026-04-28
Tier: D
Status: Accepted

## Related

References: CHE-0006

## Context

`MsgpackFileStore` persists aggregate event streams as files. A crash mid-write must leave consistent state — either old or new version, never partial. Direct write leaves truncated files on crash. WAL adds complexity and a second format. Temp-file + rename uses POSIX `rename(2)`, which atomically updates the directory entry.

## Decision

All writes in `MsgpackFileStore` go through `write_atomic`:

R1 [10]: Write all data to a temporary file then rename to the target
  path for atomic writes
R2 [10]: On rename failure clean up the temp file on a best-effort
  basis
R3 [10]: Call File::sync_all on the temp file before rename and sync
  the parent directory after rename to guarantee data durability
  across power failure
R4 [10]: Remove orphaned .msgpack.tmp files during MsgpackFileStore
  startup write recovery before create or append mutates the store

1. Serialize envelopes to bytes in memory.
2. Write bytes to `{filename}.tmp` in the store directory.
3. `rename()` the temp file to the target path.
4. `File::sync_all()` the temp file before rename.
5. `sync_all()` the parent directory after rename.
6. On rename failure, clean up the temp file (best-effort).
7. On the next write after restart, remove orphaned `.msgpack.tmp`
   files before mutating aggregate data.

Temp file naming uses `{aggregate_filename}.tmp` (e.g.,
`1.msgpack.tmp`). This is safe because:

- `append` holds a per-aggregate write lock — no two appends to the
  same aggregate run concurrently, so their temp files cannot collide.
- `create` assigns unique sequential IDs under a global mutex — no
  two creates target the same filename.

## Consequences

Crash during write leaves only the old file plus a temp file. POSIX `rename(2)` provides atomic replacement. Orphaned `.tmp` files are cleaned before the next write. Full-history rewrite remains unsuitable for long-lived aggregates.
