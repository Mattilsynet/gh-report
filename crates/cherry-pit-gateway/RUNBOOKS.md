# Operational Recovery Runbooks — `cherry-pit-gateway`

Per [CHE-0047](../../docs/adr/cherry/CHE-0047-operational-recovery-runbooks.md).

**Retirement notice (CHE-0100 L2-R2, msgpack-removal-2):** the
file-based, MessagePack-serialized event store this document was
originally written against has been deleted. Cherry-pit event
persistence is now nats/pgno-only, via `pardosa` (`.pgno`, default
backend) or `pardosa-nats`. Runbooks R1 (orphan `.msgpack.tmp`
recovery), R2 (CorruptData classification), R3 (quarantine before
repair), R4 (dead-letter record schema), and R6 (migration recovery)
were scoped to the msgpack file format and no longer apply to this
crate; they are retained below only as historical record of the
retired store's operational contract, not as current operator
guidance.

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
   `stale_lock_evidence` in `src/recovery.rs` captures this metadata
   in-process, before deleting the lock file.
3. Remove the stale lock:
   ```bash
   rm <store_dir>/.lock
   ```
4. Restart the application.

**Verification**: `stale_lock_evidence` and `StaleLockEvidence` in
`src/recovery.rs` (unit-tested independently of any store backend).

---

## Historical runbooks (retired store — CHE-0100 L2-R2)

The following procedures targeted the deleted file-based store and are
retained only for historical continuity with prior incident records that
cite them. They do not apply to the current crate surface.

### R1 — Orphan temp-file recovery (retired)

Targeted automatic removal of orphaned `.msgpack.tmp` files by the
retired store's temp-file recovery routine. No longer applicable;
nats/pgno backends do not use this temp-file-then-rename mechanism.

### R2 — CorruptData classification (retired)

Targeted `EventStore::load` returning `StoreError::CorruptData` for
truncated MessagePack bytes, aggregate ID mismatches, or sequence gaps
in `.msgpack` files. No longer applicable.

### R3 — Quarantine before repair (retired)

Targeted manual quarantine of corrupt `.msgpack` files pending repair.
No longer applicable.

### R4 — Dead-letter record schema (retired)

Documented the minimum dead-letter record fields for the msgpack store's
consumers. The schema's shape may still inform future backends, but the
v0.1 implementation pointer (`crates/cherry-pit-app/src/dead_letter.rs`)
predates this retirement and should be re-verified against the current
persistence backend before reuse.

### R6 — Migration recovery (retired)

Documented a design constraint for a hypothetical future migration tool
between the retired file-based store and an `object_store`-backed
store. Moot: the source side of that migration no longer exists.
