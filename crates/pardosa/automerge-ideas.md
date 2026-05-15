# Automerge Patterns That Could Inform Pardosa

Automerge (v0.8.0, Rust crate) is a CRDT library for collaborative local-first
applications. Despite the fundamentally different concurrency models (Pardosa is
single-writer event sourcing; Automerge is multi-writer CRDT), several Rust-level
patterns in Automerge are directly applicable to Pardosa's design.

Source: https://docs.rs/automerge/latest/automerge/

---

## 1. Transaction with Auto-Rollback on Drop — `Automerge::transact()`

Automerge's `Transaction` auto-rolls back if not committed, using `Drop`:

```rust
// Automerge pattern
doc.transact(|tx| {
    tx.put(&alice, "email", "new@example.com")?;
    Ok(result)
})?; // Rolls back on Err, commits on Ok
```

**Pardosa relevance:** Pardosa's Phase 2 `Dragline` mutations (create/update/
detach/rescue) currently have no transactional grouping. With the publish-then-apply
pattern, a failed NATS publish leaves in-memory state unchanged — but there's no way
to batch multiple mutations atomically. A `transact` closure pattern would let Pardosa
batch multiple fiber operations (e.g., create 3 fibers + update 2) into a single atomic
unit that either all publishes or all rolls back. The `Drop`-based rollback is
especially relevant given Pardosa holds an `RwLock` — it guarantees the lock is always
released cleanly even on `?` early returns.

**Phase:** 2 (Dragline + Concurrency)
**Priority:** High

---

## 2. `PatchLog` — Incremental Change Observation

Automerge's `PatchLog` records exactly what changed during an operation, so a UI or
downstream consumer can efficiently update materialized views without re-reading the
entire document:

```rust
let mut patch_log = PatchLog::active();
doc.sync().receive_sync_message_log_patches(&mut state, msg, &mut patch_log);
let patches = doc.make_patches(&mut patch_log);
// patches: Vec<Patch> — only the deltas
```

**Pardosa relevance:** Pardosa currently has `read_line` and `history` for full reads,
but no incremental change notification. A `PatchLog`-like mechanism would be valuable
for:

- **Consumer cutover** (Phase 6): Instead of replaying the full new stream, consumers
  could receive patches describing exactly what changed during migration.
- **Real-time consumers**: An append to a fiber could emit a structured patch
  (`FiberAppended { domain_id, event_id, new_state }`) rather than requiring consumers
  to poll.
- The `PatchLog::inactive()` pattern (no-op log for when you don't need patches) is
  elegant — avoids `Option<PatchLog>` branching.

**Phase:** 5-6 (Persistence + Consumer Lifecycle)
**Priority:** High

---

## 3. `ReadDoc` Trait — Unified Read Interface Across States

Automerge defines a single `ReadDoc` trait that both `Automerge` (the document) and
`Transaction` (an in-progress mutation) implement. This means you can read consistent
state whether inside or outside a transaction:

```rust
trait ReadDoc {
    fn get(&self, obj, prop) -> Result<Option<(Value, ObjId)>>;
    fn keys(&self, obj) -> Keys;
    fn values(&self, obj) -> Values;
    // ... all read methods
}
```

**Pardosa relevance:** Pardosa's `Dragline<T>` has read methods (`read`, `list`,
`history`) mixed with write methods. Extracting a `ReadDragline` trait would allow:

- Consumers to receive a read-only view without access to mutation.
- The migration code (Phase 4 steps 2-3) to hand out read access via the trait while
  the write path is locked.
- Future `PersistenceAdapter` implementations to also implement `ReadDragline` for
  reading directly from persisted state.

**Phase:** 2-3 (Dragline + Server API)
**Priority:** High

---

## 4. `fork()` / `fork_at()` — Branching at Historical Points

```rust
let fork = doc.fork();           // Branch at current state, new actor
let fork = doc.fork_at(&heads)?; // Branch at a historical point
```

**Pardosa relevance:** This maps directly to a pattern Pardosa needs for **migration
testing**: fork a `Dragline` at a point, apply a trial migration, validate the result,
then either commit or discard. Currently Pardosa's migration is one-shot (new-stream
model). A `fork_at` equivalent would enable:

- Dry-run migrations against a snapshot of the current state.
- Branching for A/B testing of migration policies.
- Since Pardosa uses `Clone` on fibers/events, `fork()` is straightforward to implement.

**Phase:** 4 (Migration)
**Priority:** Medium

---

## 5. `save_after()` — Incremental Persistence

```rust
let incremental = doc.save_after(&last_saved_heads); // Only changes since
let full = doc.save();                                 // Full snapshot
```

**Pardosa relevance:** Pardosa's genome file persistence currently writes a full
multi-message file. An incremental `save_after(last_event_id: u64)` would reduce
snapshot I/O — only serialize events appended since the last snapshot. This is
especially relevant for:

- Frequent genome file snapshots (e.g., every N events) without O(total_events) cost.
- The pattern of having both `save()` (full) and `save_after()` (incremental) maps
  cleanly to Pardosa's genome file + NATS dual-persistence model.

**Phase:** 5 (Persistence + Genome Integration)
**Priority:** Medium

---

## 6. `proptest` for Arbitrary Operation Sequences

Automerge uses `proptest` extensively (it's a dev-dependency). Pardosa has `proptest`
declared but unused (noted as L2 in pardosa.md).

Automerge's approach of generating arbitrary sequences of operations and verifying
invariants post-sequence is directly applicable to Pardosa's state machine:

```rust
// Pardosa proptest strategy
proptest! {
    fn arbitrary_transitions_preserve_invariants(
        actions in vec(arb_fiber_action(), 0..100)
    ) {
        let mut state = FiberState::Undefined;
        for action in actions {
            match transition(state, action) {
                Ok(new_state) => { state = new_state; }
                Err(_) => {} // invalid transition, skip
            }
        }
        // Invariant: state is always one of the 5 valid states
    }
}
```

**Phase:** 2 (Dragline + Concurrency)
**Priority:** Medium

---

## 7. `diff()` — Computing Deltas Between Two States

```rust
let patches = doc.diff(&before_heads, &after_heads);
```

Automerge can compute the exact set of changes between any two points in history —
forward or backward.

**Pardosa relevance:** Pardosa has `history(domain_id)` to walk a single fiber's chain,
but no way to compute what changed between two line positions or across a migration
boundary. A `diff(from_index: Index, to_index: Index)` method would be useful for:

- **Operational monitoring**: "What changed in the last N events?"
- **Migration validation**: Compare pre-migration and post-migration state.
- **Consumer catch-up**: "Give me everything since event_id X" (partially addressed by
  `event_id`-based replay, but a structured diff API would be cleaner).

**Phase:** 3+ (Server API)
**Priority:** Low

---

## 8. `Sync::State` — Per-Peer Synchronization State

Automerge tracks per-peer sync state (what the other peer has, in-flight messages) as a
separate `State` object:

```rust
let mut peer_state = sync::State::new();
doc.sync().generate_sync_message(&mut peer_state);
doc.sync().receive_sync_message(&mut peer_state, message)?;
```

**Pardosa relevance:** Pardosa's consumer cutover (Phase 6) maintains
`last_processed_event_id` per consumer — essentially the same concept. Formalizing this
as a `ConsumerState` struct (rather than ad-hoc tracking) would improve the consumer
lifecycle:

```rust
struct ConsumerState {
    last_event_id: u64,
    generation: u64,
    // could also track: pending acks, lag metrics
}
```

**Phase:** 6 (NATS Consumer Lifecycle)
**Priority:** Low

---

## 9. `apply_changes()` with Idempotency

```rust
doc.apply_changes(changes)?; // Idempotent — already-applied changes are ignored
```

Pardosa already plans idempotent replay via `event_id` dedup (pardosa-next.md design
invariant 2), but Automerge's implementation is instructive. Automerge tracks which
changes it has seen via content-addressed hashes (SHA256 of change data). Pardosa's
`event_id` monotonic counter serves the same purpose but is simpler. The key insight is
that `apply_changes` is the unified entry point for both sync and replay — Pardosa's
`PersistenceAdapter::replay()` could follow the same pattern of returning all events
and letting the `Dragline` skip already-applied ones.

**Phase:** 5 (Persistence)
**Priority:** Low (validates existing design)

---

## 10. `save_and_verify()` — Roundtrip Verification

```rust
let bytes = doc.save_and_verify()?; // Serialize, then load, then return bytes
```

This is a test/debug aid: serialize the document, immediately deserialize the result,
and verify the loaded state matches. Slow, but catches serialization bugs.

**Pardosa relevance:** A `save_and_verify` for genome file writes would catch
serialization/deserialization mismatches early — especially valuable given that
pardosa-genome uses a custom binary format with schema hashing. Could be used as a
debug-only assertion during migration file writes.

**Phase:** 5 (Persistence)
**Priority:** Low

---

## Summary: Adoption Priority

| Priority | Pattern | Pardosa Phase | Impact |
|----------|---------|---------------|--------|
| **High** | `transact()` with auto-rollback | 2 | Atomic batching of mutations under `RwLock` |
| **High** | `PatchLog` incremental observation | 5-6 | Efficient consumer notification |
| **High** | `ReadDoc` trait separation | 2-3 | Clean read/write API boundary |
| **Medium** | `fork_at()` branching | 4 | Migration dry-runs and testing |
| **Medium** | `save_after()` incremental saves | 5 | Reduced snapshot I/O cost |
| **Medium** | `proptest` arbitrary sequences | 2 | State machine fuzzing (already planned) |
| **Low** | `diff()` between states | 3+ | Operational tooling, monitoring |
| **Low** | `ConsumerState` formalization | 6 | Cleaner consumer lifecycle |
| **Low** | Idempotent `apply_changes` | 5 | Validates existing `event_id` dedup design |
| **Low** | `save_and_verify()` roundtrip | 5 | Debug aid for genome serialization |
