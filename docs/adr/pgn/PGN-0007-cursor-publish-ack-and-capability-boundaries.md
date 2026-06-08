# PGN-0007. Cursor, Publish, ACK, and Capability Boundaries

Date: 2026-06-08
Last-reviewed: 2026-06-08
Tier: B
Status: Accepted
Crates: pardosa

## Related

References: PGN-0001, PGN-0002, PGN-0004, PGN-0006

## Context

Sources rescue ADR-0011 (cursor semantics & resume), rescue ADR-0015 (publisher fallibility / re-buffer policy), and rescue ADR-0016 (least-capability reader/writer split + durable publish recovery). Compatible Solon material: PAR-0008 (publish-then-apply with durable-first semantics). The cursor is a consumer-side replay primitive (it does not rebuild frontier). Capability separation is type-level inside the runtime ring: reader components cannot name event-authoring authority. Publisher fallibility is non-fatal to the journal; pending anchors re-buffer in original roll order and persist across crashes via a writer-owned sidecar watermark.

## Decision

`Cursor<T>` is a sync `Iterator`-shaped GAT trait with `tail()`, `commit_offset(EventId)`, and `acked_offset() -> Option<EventId>`. Resume is exclusive. One cursor per source. `JournalCursor::commit_offset` persists to a sidecar (8 LE bytes, fsync per commit, no atomic rename). `FrontierPublisher::publish` returns `Result<(), PublishError>`; failed anchors halt the drain and re-buffer in roll order, bounded by `anchor_buffer_cap`. The writer-side `<journal>.publish` sidecar records the last-published anchor, fsynced after every successful publish. Two within-runtime façades — `pardosa::reader::prelude` and `pardosa::writer::prelude` — pin the capability boundary; the legacy mixed `pardosa::prelude` is removed (PGN-0009 clean break).

R1 [5]: `Cursor<T>` is a sync GAT-bearing trait with `tail`, `commit_offset`,
  and `acked_offset`; resume is exclusive (yield events whose `event_id >
  acked_offset()`); one cursor binds to exactly one event source.
R2 [5]: `JournalCursor::commit_offset` persists 8 LE bytes (`EventId.value()`)
  to a caller-supplied sidecar path, fsynced per commit; no atomic rename,
  no parent-directory fsync. Missing or torn sidecar restarts from the
  beginning, safe under exclusive resume's idempotence.
R3 [5]: `commit_offset(id)` with `id ≤ acked_offset()` is a no-op; the
  watermark cannot move backward and out-of-range commits are accepted
  without validating against the journal tail.
R4 [5]: `FrontierPublisher::publish` returns `Result<(), PublishError>`;
  failure halts the in-batch drain and re-buffers the failed anchor plus
  every later-in-batch unattempted anchor in original roll order.
R5 [5]: The pending-anchor buffer is bounded by `anchor_buffer_cap`
  (default `65_536`); overflow surfaces as
  `PardosaError::AnchorBufferOverflow { cap }` from the offending
  `commit_event` with no-op-on-`Err` (no append, no roll, no advance).
R6 [5]: Reader components receive `DraglineView<'_, T>`; `pardosa::reader::
  prelude` cannot name `Writer`, `Syncable`, `Journal::commit_event`,
  `Dragline::create`, `FrontierPublisher`, `PublishError`, or
  `AppendResult` — the boundary is type-level, not feature-flag-level.
R7 [5]: The `<journal>.publish` writer-owned watermark sidecar is fsynced
  after every successful publish; on restart, anchors with
  `event_id > watermark` are reconstructed from `.pgno` in commit order
  and republished, yielding byte-identical anchor identity.

## Consequences

+ becomes easier: typed `tail / commit_offset / acked_offset` surface for
  adopters; durable writes decoupled from publisher availability;
  exactly-once-or-retried anchor semantics across crashes; type-level
  least-capability enforcement.
− becomes harder: cross-process atomic batching of anchors (none provided);
  silently dropping anchors on publish failure (the typed `Err` surfaces
  the failure); naming writer authority from a reader handle.
risks/migration: sidecar-per-cursor multiplies on-disk state; one fsync per
  successful publish; PAR-0008 `RwLock`-style framing is the implementation
  detail under which these rescue contracts execute.
