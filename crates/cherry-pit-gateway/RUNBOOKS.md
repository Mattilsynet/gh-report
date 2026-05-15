# Operational Recovery Runbooks — `cherry-pit-gateway`

Per [CHE-0047](../../docs/adr/cherry/CHE-0047-operational-recovery-runbooks.md).
All procedures target `MsgpackFileStore` on a single-machine deployment.

---

## R1 — Orphan temp-file recovery

**When**: after a crash or power loss, `.msgpack.tmp` files may remain in the
store directory from interrupted atomic writes.

**Automated behaviour**: `MsgpackFileStore` removes orphaned `.msgpack.tmp`
files automatically on first `create` or `append` after construction
(`recover_temp_files` at `msgpack_file.rs:269`). No operator intervention
is required under normal operation.

**Manual procedure** (if automated recovery is unavailable):

1. Stop the application process owning the store directory.
2. List orphaned temp files:
   ```bash
   ls <store_dir>/*.msgpack.tmp
   ```
3. Verify each `.msgpack.tmp` is not referenced by any valid `.msgpack` file
   (it never is — temp files exist only before the atomic rename).
4. Remove orphaned temp files:
   ```bash
   rm <store_dir>/*.msgpack.tmp
   ```
5. Restart the application. The store will re-scan the directory on first
   `create` call and assign IDs correctly.

**Verification**: after restart, `ls <store_dir>/*.msgpack.tmp` returns no
results. Tested by `orphaned_temp_file_removed_on_next_write` in
`msgpack_file.rs`.

> **Verified by walk-through at `dd979886d5ca7f9faaf230bbf9f59a2d6fd76440`** (R1: orphan `.msgpack.tmp` removal).
> Walk: unit-test invocation `cargo test -p cherry-pit-gateway orphaned_temp_file_removed_on_next_write` (gh-report binary lacks a `--store-dir` override on the main daemon path at this SHA, and `gh auth` credentials are unavailable in this environment, so the documented test-based fallback was used).
> Reference test: `orphaned_temp_file_removed_on_next_write` at `msgpack_file.rs:1778`.

---

## R2 — CorruptData classification

**When**: `EventStore::load` returns `StoreError::CorruptData`.

**Root causes**: truncated MessagePack bytes, aggregate ID mismatch between
filename and envelope content, sequence gaps in the event stream, zero-sequence
envelopes.

**Procedure**:

1. Note the aggregate ID from the error message.
2. Copy the corrupt file for forensic analysis:
   ```bash
   cp <store_dir>/<id>.msgpack <quarantine_dir>/<id>.msgpack.corrupt
   ```
3. Inspect with a MessagePack decoder (e.g. `msgpack-tools`, `python -c
   "import msgpack; …"`). Identify whether corruption is truncation, field
   mismatch, or sequence violation.
4. If recoverable: reconstruct a valid stream from upstream event sources
   (if available) and write it to `<store_dir>/<id>.msgpack`. The store
   will validate on next `load`.
5. If unrecoverable: follow R3 (quarantine) and R4 (dead-letter record).

**Verification**: `load_rejects_sequence_gap`, `load_rejects_aggregate_id_mismatch`,
`corrupt_file_returns_error`, `old_format_with_zero_sequence_rejected_on_load`
in `msgpack_file.rs`.

---

## R3 — Quarantine before repair

**When**: a corrupt `.msgpack` file is identified (via R2) and repair is needed.

**Procedure**:

1. Stop the application (or ensure single-writer fencing prevents concurrent
   access).
2. Create a quarantine directory:
   ```bash
   mkdir -p <store_dir>/quarantine
   ```
3. Move the corrupt file:
   ```bash
   mv <store_dir>/<id>.msgpack <store_dir>/quarantine/<id>.msgpack.<timestamp>
   ```
4. Record the quarantine action in an operator log (or dead-letter record per R4).
5. Only after repair tooling produces a validated replacement stream, write it
   back to `<store_dir>/<id>.msgpack`.
6. Restart and verify: `load` for that aggregate ID must succeed.

**Note**: v0.1 has no in-process quarantine tooling. This is a manual operator
procedure. Future versions may automate quarantine-on-error.

**Doctrine pointer**: per [CHE-0051:R7](../../docs/adr/cherry/CHE-0051-cherry-pit-agent-design.md) line 73,
"the durability decision belongs to the consumer's operational story
(CHE-0047 runbook scope)". v0.1 gateway does not own this surface.

---

## R4 — Dead-letter record schema

**When**: an event cannot be processed or persisted and must be recorded for
later resolution.

**Record schema** (JSON or Markdown):

```json
{
  "event_id": "<UUID v7>",
  "aggregate_id": "<u64>",
  "sequence": "<u64>",
  "correlation_id": "<UUID v7 | null>",
  "causation_id": "<UUID v7 | null>",
  "error_category": "CorruptData | Infrastructure | ConcurrencyConflict",
  "error_detail": "<error message>",
  "operator_action": "<description of resolution>",
  "timestamp": "<ISO 8601>",
  "operator": "<name or system>"
}
```

**Worked example**:

```json
{
  "event_id": "0193a3e8-8000-7cde-8f01-23456789abcd",
  "aggregate_id": "42",
  "sequence": "3",
  "correlation_id": "aabbccdd-eeff-7122-8344-556677889900",
  "causation_id": null,
  "error_category": "CorruptData",
  "error_detail": "sequence gap: expected 3, found 5 at envelope index 2",
  "operator_action": "Quarantined to quarantine/42.msgpack.20260507T120000Z; reconstructed from upstream replay",
  "timestamp": "2026-05-07T12:00:00Z",
  "operator": "ops-team"
}
```

**Note**: v0.1 has no in-process dead-letter handler. This schema documents
the minimum fields an operator must capture. Future versions may automate
dead-letter recording.

**v0.1 implementation pointer**: dead-letter sink lives in the consumer
crate per [CHE-0051:R7](../../docs/adr/cherry/CHE-0051-cherry-pit-agent-design.md).
The agent ships `DeadLetterSink`, `DeadLetterRecord`, and
`TracingDeadLetterSink` at `crates/cherry-pit-agent/src/dead_letter.rs`
(as of commit `dd7c3b1`). Gateway does not implement R4; the schema
above documents the minimum fields any consumer's persistence path
must preserve.

<!--
PM-3 mitigation: the path cite above is commit-pinned (`as of <sha>`)
to avoid stale-name drift if the agent module is renamed. When this
RUNBOOKS section is next revised, re-resolve the path at the current
HEAD before updating the SHA.
-->

---

## R5 — Stale-lock recovery

**When**: the application crashes while holding the advisory `flock` on
`<store_dir>/.lock`, and a new process cannot acquire the lock.

**Background**: advisory locks (`flock(2)`) are released automatically when
the file descriptor is closed (including on process crash). In practice,
stale locks are rare. They occur only if the filesystem does not properly
release locks (e.g. NFS without proper lock management).

**Procedure**:

1. Verify the owning process is truly dead:
   ```bash
   lsof <store_dir>/.lock    # or: fuser <store_dir>/.lock
   ```
2. If no process holds the lock, the lock file is stale. Record evidence:
   ```bash
   ls -la <store_dir>/.lock
   stat <store_dir>/.lock
   ```
3. Remove the stale lock:
   ```bash
   rm <store_dir>/.lock
   ```
4. Restart the application. `ensure_fenced` will create a new `.lock` file
   and acquire it.

**Verification**: `second_store_same_dir_fails_with_store_locked` and
`lock_released_on_drop_allows_reacquisition` in `msgpack_file.rs`.

---

## R6 — Migration recovery

**When**: a future migration tool fails mid-stream while copying events
between storage backends (e.g. `MsgpackFileStore` → `object_store`-backed).

**v0.1 status**: no migration tool exists. This runbook documents the
design constraint for any future implementation.

**Required durable state** (per CHE-0047:R6):

| Field | Purpose |
|-------|---------|
| `phase` | Current migration phase (e.g. `copying`, `validating`, `switching`, `cleanup`) |
| `source_stream` | Path or identifier of the source aggregate stream |
| `target_stream` | Path or identifier of the target aggregate stream |
| `last_copied_sequence` | Highest sequence number successfully copied |
| `cleanup_ownership` | Which process/operator owns the cleanup responsibility |

**Recovery procedure** (future):

1. Read the durable migration state.
2. Resume from `last_copied_sequence + 1`.
3. After all events are copied, validate the target stream (`validate_stream`).
4. Switch reads to the target backend.
5. Mark source as archived (do not delete — quarantine per R3 principles).

**Note**: any migration implementation must satisfy CHE-0047:R6 by
recording this state durably before each phase transition.

**Doctrine pointer**: per [CHE-0051:R7](../../docs/adr/cherry/CHE-0051-cherry-pit-agent-design.md),
migration-recovery durability sits in the consumer's operational story,
not in gateway.
