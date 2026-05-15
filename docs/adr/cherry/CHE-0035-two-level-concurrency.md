# CHE-0035. Two-Level Concurrency Architecture

Date: 2026-04-25
Last-reviewed: 2026-04-28
Tier: D
Status: Accepted

## Related

References: CHE-0006

## Context

`MsgpackFileStore` must handle concurrent operations safely. Multiple aggregates can be written simultaneously, but two appends to the same aggregate must be serialized. New aggregate ID assignment must be globally unique. Options: global lock (simple, no parallelism), per-aggregate lock (fine-grained), or lock-free (complex with file I/O).

## Decision

`MsgpackFileStore` uses a two-level concurrency architecture:

R1 [10]: Use a global mutex for aggregate ID assignment to guarantee
  uniqueness
R2 [10]: Use per-aggregate write locks via scc::HashMap for
  fine-grained concurrency between different aggregates
R3 [10]: Reads are lock-free because writes are atomic via temp file
  plus rename

1. **Global ID mutex** (`tokio::sync::Mutex<Option<u64>>`) — held only
   during aggregate ID assignment in `create`. Serializes ID
   generation to guarantee uniqueness. Lazy initialization: `None`
   on first access triggers `scan_max_id()` to read the directory.

2. **Per-aggregate write locks** (`scc::HashMap<u64,
   Arc<tokio::sync::Mutex<()>>>`) — `scc::HashMap` is a lock-free
   concurrent hash map. Two access patterns:
   - Fast path: `read_sync` — lock-free read for existing entries.
   - Slow path: `entry_sync` + `or_insert_with` — fine-grained insert
     for new entries.
   Each aggregate gets its own `tokio::sync::Mutex<()>` wrapped in
   `Arc` for sharing across tasks.

3. **Lock-free reads** — `load()` reads files directly without
   acquiring any lock. This is safe because writes are atomic (temp
   file + rename) — a concurrent read sees either the old or new
   version, never a partial write.

`create` does NOT acquire a per-aggregate write lock. This is safe
because the global ID mutex guarantees the assigned ID is unique — no
other operation can target a freshly assigned ID. The write to disk
happens after the mutex is released but before any other operation can
know the new ID.

## Consequences

- Different aggregates can be read and written concurrently without contention.
- Same-aggregate writes are serialized, preventing read-check-write races.
- Reads never block — concurrent reads see the pre-write state.
- The `scc::HashMap` grows monotonically — locks are never removed.
- `create` without per-aggregate lock is safe because temp file naming is coupled to sequential ID uniqueness.
