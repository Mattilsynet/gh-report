# CHE-0036. File-Per-Stream Full-Rewrite Storage Model

Date: 2026-04-25
Last-reviewed: 2026-04-28
Tier: D
Status: Accepted

## Related

References: CHE-0032

## Context

`MsgpackFileStore<E>` needs a storage topology and persistence strategy. Three topology options: single file for all aggregates, file per instance, or directory with segments. Three persistence strategies: append-only (impossible with MessagePack's fixed array length), full rewrite, or WAL with compaction.

## Decision

One file per aggregate instance. Full rewrite on every append.

R1 [10]: Store one .msgpack file per aggregate instance containing the
  complete event history as Vec<EventEnvelope<E>>
R2 [10]: Rewrite the entire aggregate history on every append
  operation

### File layout

```text
store/
  1.msgpack     # aggregate 1: Vec<EventEnvelope<E>>
  2.msgpack     # aggregate 2: Vec<EventEnvelope<E>>
  ...
```

Each file contains the complete event history for one aggregate,
serialized as a single `Vec<EventEnvelope<E>>` in MessagePack named
format. The filename is the aggregate ID (a `u64`).

### Append flow

```
append(id, expected_sequence, events):
  1. Acquire per-aggregate write lock
  2. Read entire file → Vec<EventEnvelope<E>>
  3. Check optimistic concurrency (expected_sequence)
  4. Build new envelopes (UUID v7, sequence, timestamp)
  5. Extend the vector with new envelopes
  6. Serialize entire vector to bytes
  7. Write to {id}.msgpack.tmp
  8. Rename to {id}.msgpack (atomic, CHE-0032)
```

### Load flow

```
load(id):
  1. Read entire file → bytes
  2. Deserialize → Vec<EventEnvelope<E>>
  3. Return (empty Vec if file not found)
```

## Consequences

- **O(n) write per append** — rewrites entire history. Production should use a database store.
- **O(1) file reads per load** — single file read, no index.
- **File count equals aggregate count** — scales to ~1M files; sharding needed beyond that.
- **No partial reads** — consistent with CHE-0037.
- Atomic rename (CHE-0032) ensures readers never see partial writes.
