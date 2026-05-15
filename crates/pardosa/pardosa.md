# Pardosa — Research & Design Notes

Pardosa is an EDA storage layer implementing **fiber semantics** in Rust. It provides Event Carried State Transfer (ECST) with correctness, auditability, and deletion policy as first-class concerns.

## Origin

- **fiber-semantics** repo: defines the conceptual model — fibers, lines, draglines, migrations, and the per-fiber state machine (5 states: Undefined, Defined, Detached, Purged, Locked).
- **web-service-gin** repo: Go prototype implementing `pardosa.Server[T]` with generics, a `Dragline[T]` append-only log backed by `[]Event[T]`, and a `map[DomainIdentity]Fiber` lookup. NATS/JetStream persistence is stubbed but not wired.

## Core Concepts (from fiber-semantics)

| Concept | Description |
|---------|-------------|
| **Event** | Immutable fact with header: `Timestamp`, `DomainId`, `Detached`, `Precursor`, `DomainEvent` payload |
| **Fiber** | Singly linked list of events sharing a `DomainId` — the history of one entity/activity |
| **Line** | Append-only array of interleaved fibers. Locked between migrations |
| **Dragline** | A protected Line with a `LookupFiber` index for O(1) fiber head access |
| **Migration** | Version transition: enables schema upgrades, deletion policies (Keep, LockAndPrune, Purge) |

### Fiber State Machine

5 states, 10 transitions. Application operations and migration operations are separated by design.

#### States

| State | Description |
|-------|-------------|
| **Undefined** | DomainId has never existed |
| **Defined** | Fiber is active, key exists |
| **Detached** | Fiber is soft-deleted |
| **Purged** | Fiber only exists on optional audit trail. Key can be reused |
| **Locked** | Fiber only exists on optional audit trail. Key can NOT be reused |

#### Transitions

```
Undefined  --Create-->                Defined
Defined    --Update-->                Defined
Defined    --Detach-->                Detached
Detached   --Rescue-->                Defined
Detached   --Migrate(Keep)-->         Detached
Detached   --Migrate(Purge)-->        Purged
Detached   --Migrate(LockAndPrune)--> Locked
Purged     --Create-->                Defined
Locked     --Rescue-->                Defined
Locked     --Migrate(Purge)-->        Purged
```

#### Application operations (between migrations)

- **Create**: Undefined → Defined, Purged → Defined
- **Update**: Defined → Defined
- **Detach**: Defined → Detached (soft delete)
- **Rescue**: Detached → Defined, Locked → Defined (history lost on Locked, fresh start)

#### Migration operations (during migration only)

- **Migrate(Keep)**: Detached → Detached (fiber survives migration unchanged, remains soft-deleted)
- **Migrate(Purge)**: Detached → Purged, Locked → Purged (fiber removed from line, retained on optional audit trail, key reusable)
- **Migrate(LockAndPrune)**: Detached → Locked (fiber pruned to last event, removed from line, retained on optional audit trail, key not reusable except via Rescue)

#### Semantics

- **Defined fibers are implicitly kept** during migrations — no explicit Migrate(Keep) needed.
- **Locked vs Purged**: Both remove the fiber from the line. Locked prevents key reuse via Create but allows Rescue (original entity revival, history lost). Purged allows key reuse via Create (new entity).
- **Locked → Rescue**: History is lost. The rescued fiber starts fresh with no precursor from pruned events.
- **Locked → Migrate(Purge)**: Escalation path. A locked fiber can be fully purged in a subsequent migration, making the key reusable.

#### Test matrix

5 states × 7 action types (Create, Update, Detach, Rescue, Migrate(Keep), Migrate(Purge), Migrate(LockAndPrune)) = 35 pairs. 10 valid, 25 invalid.

#### Notes

- **Undefined is implicit absence** — no fiber entry exists in LookupFiber. Not a stored state.
- **Migrations are per-fiber decisions within a line-wide migration pass.** Each detached or locked fiber gets an individual migration policy applied during the pass. Defined fibers are implicitly kept. Undefined entries are skipped.

### Operations

- **Mutating**: Create, Update, Detach, Rescue
- **Migration**: Migrate(Keep), Migrate(Purge), Migrate(LockAndPrune)
- **Read**: Read, ReadWithDeleted, List, ListWithDeleted, History, ReadLine

## Go Prototype Analysis

Key types from `web-service-gin/pkg/pardosa/`:

```go
type Server[T comparable] struct {
    domainIdCounter DomainIdentity
    dragline        Dragline[T]
}

type Dragline[T comparable] struct {
    Line        []Event[T]
    LookupFiber map[DomainIdentity]Fiber
}

type Event[T comparable] struct {
    Timestamp   int64
    DomainId    DomainIdentity
    Detached    bool
    Precursor   Index
    DomainEvent T
}

type Fiber struct {
    Anchor  Index
    Len     uint64
    Current Index
}
```

### Known issues in Go prototype

- `List`/`ListWithDeleted` assume monotonically increasing DomainId — broken by design
- No concurrency (TODO: RWMutex)
- No stream persistence (TODO: NATS/JetStream write)
- Anchor always at start of fiber, should be at `Len % n`
- No migration implementation yet

## Rust Implementation Plan

Superseded by **Resolved Decisions** and **Build Plan** sections below. Retained for reference.

<details>
<summary>Original plan (superseded)</summary>

### Type Mapping

| Go | Rust |
|----|------|
| `Server[T comparable]` | `Server<T: Clone + PartialEq>` |
| `Dragline[T]` | `Dragline<T>` with `Vec<Event<T>>` + `HashMap<DomainId, Fiber>` |
| `Index int64` | `type Index = i64` or newtype `Index(i64)` |
| `DomainIdentity uint64` | `type DomainId = u64` or newtype |
| `Event[T]` | `struct Event<T>` with `serde::Serialize + Deserialize` |
| RWMutex | `RwLock<Dragline<T>>` or `tokio::sync::RwLock` for async |

### Crate Dependencies (candidates)

| Crate | Purpose | Notes |
|-------|---------|-------|
| **`async-nats`** | NATS/JetStream persistence | Official client, v0.47+, Tokio-based. Replaces deprecated sync `nats` crate |
| **`serde` + `serde_json`** | Serialization | For DomainEvent payloads and persistence |
| **`tokio`** | Async runtime | Required by async-nats |
| **`thiserror`** | Error types | For `TransitionError` enum |

State machine is hand-rolled with exhaustive enum matching — no external crate. This keeps the state machine as inspectable data: a single `TRANSITIONS` table drives both runtime logic and DOT/Graphviz visualization.

### Architecture Sketch

```
pardosa/
├── Cargo.toml
├── src/
│   ├── lib.rs          # public API: Server<T>
│   ├── event.rs        # Event<T>, DomainId, Index types
│   ├── fiber.rs        # Fiber struct
│   ├── fiber_state.rs  # FiberState, FiberAction, transition(), TRANSITIONS table
│   ├── dot.rs          # DOT/Graphviz output from TRANSITIONS table
│   ├── dragline.rs     # Dragline<T>: Line + LookupFiber
│   ├── migration.rs    # MigrationContext, migration-only API surface
│   └── persistence.rs  # NATS/JetStream adapter (async-nats)
```

### Implementation Priorities

1. **Fiber state machine** — FiberState (5 variants), FiberAction (Create, Update, Detach, Rescue, Migrate(policy)), transition function, TRANSITIONS table, DOT visualization
2. **Core types** — Event, Fiber, DomainId, Index (newtype over u64, Option for no-precursor), Dragline
3. **Server API** — Create, Read, Update, Detach, Rescue, History, ReadLine
4. **Concurrency** — `RwLock<Dragline<T>>`
5. **Fix List operations** — iterate `LookupFiber` keys instead of assuming monotonic IDs
6. **Migrations** — MigrationContext gating, Keep/LockAndPrune/Purge with line reindexing
7. **Persistence** — async-nats JetStream integration

</details>

### Relevant Rust Ecosystem

**Event sourcing crates** (for reference, not direct use):

- `cqrs-es` — lightweight CQRS+ES framework, serverless-oriented
- `esrs` — Postgres-backed ES by Prima.it
- `thalo` — ES with Postgres+Kafka, includes schema DSL

**Append-only log patterns**:

- Bitcask pattern: append-only write log + in-memory HashMap index — closest match to Pardosa's `Line` + `LookupFiber` design
- `nebari` — transactional append-only KV in pure Rust
- Segmented log pattern: <https://arindas.github.io/blog/segmented-log-rust/> — shares the append-only invariant but Pardosa does not segment: single flat array, no rotation/compaction, policy-driven migrations instead

**State machines** (reference, not used — hand-rolled approach chosen for inspectability and DOT visualization):

- `statig` — hierarchical, generic, async, `no_std`
- `rust-fsm` — simpler DSL macro approach
- `sm` — compile-time validated, less maintained

## Resolved Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| DomainId type | `u64`, newtype `DomainId(u64)` | Simplicity; string conversion deferred |
| DomainId generation | Server-assigned monotonic counter | Eliminates concurrent-create races |
| Purged→Create reuse | Caller specifies purged DomainId; counter does NOT advance | Enables key reuse without wasting IDs |
| Index type | Newtype `Index(u64)` with `checked_next()` | Prevents negative values, arithmetic errors |
| No-precursor | `Option<Index>` (None = first event in fiber) | Type-safe sentinel |
| Crate type | Library crate only | No binary |
| Persistence failure | Reject writes when NATS unavailable (strict consistency) | No split-brain risk |
| Locked→Rescue | Gated behind `acknowledge_data_loss: bool` parameter | Prevents accidental history destruction |
| Audit trail | Deferred / out of scope | Simplifies initial implementation |
| Concurrency | `tokio::sync::RwLock` (not std) | Lock held across async NATS publish |
| Serialization | JSON via `serde_json` | MessagePack/protobuf are future optimizations |
| List ordering | Unspecified (`HashMap` iteration) | Document; switch to `BTreeMap` if needed |
| Anchor stride | Deferred | No `Len % n` optimization initially |
| Trait bounds on `T` | `Clone + Serialize + Deserialize` from day one | Adding serde later is breaking |

## Build Plan

### Error Taxonomy

```rust
enum PardosaError {
    InvalidTransition { state: FiberState, action: FiberAction },
    NatsUnavailable,
    MigrationInProgress,
    AcknowledgmentRequired,       // Locked→Rescue without acknowledge_data_loss
    IdNotPurged(DomainId),        // Purged→Create reuse with non-purged ID
    IdAlreadyExists(DomainId),    // Create with existing DomainId
    FiberNotFound(DomainId),      // Read/Update/Detach on Undefined
    IndexOverflow,
}
```

### Architecture

```
pardosa/
├── Cargo.toml
├── src/
│   ├── lib.rs           # Server<T>, public API re-exports
│   ├── error.rs         # PardosaError enum
│   ├── event.rs         # Index, DomainId, Event<T>
│   ├── fiber.rs         # Fiber struct
│   ├── fiber_state.rs   # FiberState, FiberAction, TRANSITIONS, transition()
│   ├── dot.rs           # DOT visualization from TRANSITIONS
│   ├── dragline.rs      # Dragline<T>, CRUD, reads, concurrency
│   ├── migration.rs     # MigrationContext, reindexing
│   └── persistence.rs   # NATS/JetStream adapter
```

### Phase 1 — State machine + core types ✅ COMPLETE

| # | File | Action | Status |
|---|------|--------|--------|
| 1 | `Cargo.toml` | Create lib crate. Deps: `serde`, `serde_json`, `thiserror`. Dev-deps: `proptest`. `tokio` deferred to Phase 2 | ✅ |
| 2 | `src/error.rs` | `PardosaError` enum with `thiserror` derives. Added `DomainIdOverflow` variant | ✅ |
| 3 | `src/event.rs` | `Index(u64)` newtype with `checked_next()`. `DomainId(u64)` newtype. `Event<T>` — trait bounds on impls not struct | ✅ |
| 4 | `src/fiber.rs` | `Fiber` with private fields, constructor with invariant checks, `advance()` method | ✅ |
| 5 | `src/fiber_state.rs` | `FiberState`, `FiberAction`, `MigrationPolicy` — all with Serialize/Deserialize. `TRANSITIONS` table, `transition()` | ✅ |
| 6 | `src/dot.rs` | DOT/Graphviz output generated from `TRANSITIONS` table | ✅ |
| 7 | Tests | 24 tests: exhaustive 35-pair matrix, overflow, serde roundtrip, duplicate key check, DOT verification | ✅ |

### Phase 2 — Dragline + concurrency

| # | File | Action |
|---|------|--------|
| 8 | `src/dragline.rs` | `Dragline<T> { line: Vec<Event<T>>, lookup: HashMap<DomainId, (Fiber, FiberState)>, purged_ids: HashSet<DomainId>, next_id: DomainId, migrating: bool }` |
| 9 | `src/dragline.rs` | CRUD: `create`, `update`, `detach`, `rescue(acknowledge_data_loss)` — each validates transition, appends to line, updates lookup |
| 10 | `src/dragline.rs` | Reads: `read`, `read_with_deleted`, `list`, `list_with_deleted`, `history`, `read_line` |
| 11 | Concurrency | Wrap in `tokio::sync::RwLock<Dragline<T>>` — publish-under-lock since NATS publish is async |
| 12 | Tests | Concurrent read/write, lifecycle end-to-end, purged-ID validation (reuse valid, reject non-purged, reject non-existent) |

### Phase 3 — Server API

| # | File | Action |
|---|------|--------|
| 13 | `src/lib.rs` | `Server<T>` struct wrapping `RwLock<Dragline<T>>`. Public API methods. Migration-in-progress rejection |
| 14 | Tests | API integration, migration rejection under concurrent load |

### Phase 4 — Migration

| # | File | Action |
|---|------|--------|
| 15 | `src/migration.rs` | `MigrationContext { version: u64, policies: HashMap<DomainId, MigrationPolicy> }`. Write-lock, set `migrating=true`, apply per-fiber policies, single-pass line rebuild with index remap table, update all `Fiber.anchor`/`current` + event `precursor` values, set `migrating=false` |
| 16 | Tests | Reindex integrity (surviving precursor chains valid), `Purged→Create→Detach→Purge→Create` multi-generation cycle, concurrent app-op rejection during migration |

### Phase 5 — NATS/JetStream persistence

| # | File | Action |
|---|------|--------|
| 17 | `Cargo.toml` | Move `async-nats` to regular dep |
| 18 | `src/persistence.rs` | JetStream adapter. Stream: `PARDOSA_{name}`, subject: `pardosa.{name}.events`. Each mutation = one publish. Startup replay: `DeliverPolicy::All`, rebuild `Dragline` from journal |
| 19 | `src/dragline.rs` | Inject persistence adapter. Publish first, apply on ACK. On failure → `NatsUnavailable` |
| 20 | Tests | Serde round-trip, publish-failure rollback (in-memory unchanged), startup replay idempotency, NATS reconnection |

### Design Invariants

1. **Atomic publish-then-apply**: Each mutation = one NATS publish. Apply to in-memory state only after publish ACK. No compound operations at the lib layer.
2. **Migration-in-progress flag inside RwLock**: Lives inside `Dragline`, checked under lock. Write-lock acquisition at migration start drains in-flight operations.
3. **Purged-ID tombstone set**: `HashSet<DomainId>` tracks purged IDs. Populated on startup replay, updated on Purge and Create-reuse. Validates caller-supplied reuse IDs.
4. **State machine as data**: `TRANSITIONS` table drives runtime logic and DOT visualization. No separate encoding.

### Open Items (non-blocking, deferred)

- `PartialEq` bound on `T`: verify if any operation compares payloads; drop if unused
- `BTreeMap` for ordered listing if needed
- Anchor stride (`Len % n`) optimization
- Audit trail as separate NATS stream consumer
- MessagePack/protobuf serialization
- Migration versioning scheme details

## Distributed Systems Review — Improvement Plan

Evaluation performed from a distributed systems design perspective. Findings organized by risk level with concrete remediation steps.

### High-Risk Issues

#### H1. `debug_assert!` guards correctness invariants in `Fiber::new`

The `len >= 1` and `current >= anchor` checks vanish in release builds. A malformed `Fiber` silently corrupts the data model — every downstream operation (advance, read, migration reindex) assumes these hold.

**Remediation:** Replace `debug_assert!` with `assert!` or change `Fiber::new` to return `Result<Fiber, PardosaError>`. Add a `FiberInvariantViolation` error variant if using `Result`.

**When:** Before Phase 2. All downstream code depends on these invariants.

#### H2. Publish-then-apply assumes exactly-once, but JetStream provides at-least-once

If publish succeeds but ACK is lost (network timeout, partition), the caller receives `NatsUnavailable`, in-memory state is unchanged, but the event exists in JetStream. On startup replay, a phantom event appears — divergence between observed failure and durable state.

Without an idempotency key on `Event<T>`, deduplication during replay is impossible. Adding this field later is a breaking serialization change.

**Remediation:** Add `event_id: u64` to `Event<T>` now (Phase 1 amendment). Assign monotonically at append time. Use JetStream `Nats-Msg-Id` header for publish-side deduplication. Deduplicate on replay by `event_id`.

**When:** Phase 2 prerequisite (first task). Serialization format must be stable before persistence is wired.

#### H3. `Fiber::advance` has no bounds check

A caller can set `new_current` to a value less than `current`, violating the singly-linked-list invariant silently. No test covers this path.

**Remediation:** Add `assert!(new_current.value() > self.current.value())` or return `Result`. Strictly `>` (not `>=`) — each event occupies a unique index in the append-only line. Add test for decreasing index.

**When:** Before Phase 2.

#### H4. Single-writer assumption is implicit but critical

The publish-then-apply model with `RwLock<Dragline<T>>` assumes one `Server<T>` instance per NATS stream. Multiple instances writing to the same stream create divergent in-memory states with no reconciliation mechanism.

**Remediation:** Document constraint explicitly in `pardosa.md` and `Server<T>` doc comments. If multi-instance is ever needed, requires fencing token or leader election — fundamentally different architecture.

**When:** Before Phase 2. Affects all design decisions downstream.

### Medium-Risk Issues

#### M1. RwLock held across async NATS publish

Under high write throughput, lock contention serializes all mutations behind network latency. No backpressure or circuit breaker.

**Remediation (Phase 2):**
- (a) Add write-lock acquisition timeout to prevent unbounded blocking.
- (b) Bounded channel between app ops and NATS publish for backpressure.
- (c) Circuit breaker: after N consecutive `NatsUnavailable`, enter degraded mode (serve reads, reject writes, alert).

#### M2. Migration holds write-lock for entire duration

Line rebuild with reindexing is O(n) over all events. Large lines block writes for seconds+. Note: coupled with M5 — chunked migration changes crash-recovery semantics; these must be designed together in Phase 4.

**Remediation (Phase 4):** Consider chunked migration — process N fibers per lock acquisition cycle. Add progress callback. Document expected downtime as function of line size.

#### M3. `Event<T>` fields are `pub`

Events are immutable by design, but public fields allow post-construction mutation, undermining the append-only invariant at the API boundary.

**Remediation (Phase 2):** Make fields private, add constructor + accessors. Or document the immutability contract with a `// SAFETY:` comment explaining the invariant.

#### M4. No event schema versioning

`T` is generic but no upcasting mechanism exists for evolving payloads during replay. First schema change will break replay.

**Remediation (Phase 5):** Consider `EventEnvelope<T>` wrapper with `schema_version: u32`. Require `T` to implement an upcast trait. Alternatively, handle at the application layer with serde `#[serde(default)]` and deny-unknown-fields.

#### M5. Migration crash-recovery semantics unspecified

If process crashes mid-rebuild, is the old line still in NATS? Is migration replayable/idempotent? Note: coupled with M2 — if migration is chunked, recovery semantics change fundamentally.

**Remediation (Phase 4):** Specify recovery semantics. Old events remain in JetStream; migration must be idempotent. Consider writing a `MigrationStarted` / `MigrationCompleted` sentinel event to the stream.

### Low-Risk Issues

#### L1. O(n) transition lookup

Linear scan of 10 entries. Negligible at N=10 but a `match` statement would give compile-time exhaustiveness — stronger guarantee than the runtime `no_duplicate_state_action_pairs` test.

#### L2. `proptest` declared but unused

Candidates: arbitrary state-action sequences preserve valid states; arbitrary event sequences maintain fiber chain integrity; fuzz `Fiber::new` and `advance` inputs.

#### L3. Unbounded `purged_ids: HashSet<DomainId>`

Grows monotonically. Acceptable for most workloads. Consider compaction during migration if entity churn is high.

#### L4. `Vec<Event<T>>` unbounded memory growth

Migration with Purge/LockAndPrune is the planned compaction. Document that memory is bounded by events-since-last-migration. For very large deployments, consider segmented log with memory-mapped segments.

#### L5. Wall-clock `timestamp: i64`

Adequate for single-writer (see H4). If multi-writer is ever needed, requires hybrid logical clocks (HLC) or vector clocks.

### Test Gaps

| Gap | Phase | Priority |
|-----|-------|----------|
| `Fiber::new` with `len = 0` (release build) | Pre-2 | High |
| `Fiber::advance` with decreasing index | Pre-2 | High |
| Concurrent create with same `DomainId` | 2 | High |
| Publish-succeeds-ACK-lost simulation | 5 | High |
| Replay produces identical state with duplicates | 5 | High |
| `DomainId` counter overflow at `Server` level | 2 | Medium |
| `proptest` arbitrary transition sequences | 2 | Medium |
| Migration interrupted mid-rebuild (crash recovery) | 4 | Medium |

### Open Questions

1. **Event ID format**: `u64` monotonic (simpler, smaller, single-writer) or `Uuid` (globally unique, no coordination)? Single-writer (H4) favors `u64`.
2. **Migration downtime tolerance**: Blocking all writes acceptable, or need online chunked migration?
3. **Schema evolution frequency**: Determines whether versioned envelope is worth the complexity now.
4. **Expected line size**: Determines whether `Vec<Event<T>>` is viable or segmentation is needed near-term.
5. **Multi-instance future**: If single-writer (H4) constraint is ever lifted, need fencing token or leader election — fundamentally different architecture. Validate this is acceptable for all target deployments.
