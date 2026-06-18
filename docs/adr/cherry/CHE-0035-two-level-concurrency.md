# CHE-0035. Two-Level Concurrency Architecture

Date: 2026-04-25
Last-reviewed: 2026-06-19
Tier: D
Status: Accepted

## Related

References: CHE-0006

## Context

`MsgpackFileStore` must handle concurrent operations safely. Multiple aggregates can be written simultaneously, but two appends to the same aggregate must be serialized. New aggregate ID assignment must be globally unique. Options: global lock (simple, no parallelism), per-aggregate lock (fine-grained), or lock-free (complex with file I/O).

## Decision

`MsgpackFileStore` uses a two-level concurrency architecture:

R1 [10]: Seed aggregate ID assignment from a one-shot directory scan
  held in a `tokio::sync::OnceCell`, with per-call allocation via an
  inner `AtomicU64` that holds no guard across `.await`
R2 [10]: Use per-aggregate write locks via scc::HashMap for
  fine-grained concurrency between different aggregates
R3 [10]: Reads are lock-free because writes are atomic via temp file
  plus rename

1. **Lock-free ID counter** (`tokio::sync::OnceCell<AtomicU64>`) — the
   `OnceCell` ensures `scan_max_id()` runs at most once per store
   instance to seed the counter from the directory; the inner
   `AtomicU64` hands out unique IDs via atomic increment without
   holding a lock across `.await` points.

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
because the atomic ID counter guarantees the assigned ID is unique — no
other operation can target a freshly assigned ID. The write to disk
happens after the ID is allocated but before any other operation can
know the new ID.

## Consequences

- Different aggregates can be read and written concurrently without contention.
- Same-aggregate writes are serialized, preventing read-check-write races.
- Reads never block — concurrent reads see the pre-write state.
- The `scc::HashMap` grows monotonically — locks are never removed.
- `create` without per-aggregate lock is safe because temp file naming is coupled to sequential ID uniqueness.
