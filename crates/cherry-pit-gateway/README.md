# cherry-pit-gateway

Infrastructure implementations for cherry-pit: event stores.

Provides [`MsgpackFileStore`] — a file-based, MessagePack-serialized event store
with atomic writes (CHE-0032), process-level fencing (CHE-0043), optimistic
concurrency (CHE-0006, CHE-0035), and operational recovery procedures (CHE-0047).

## Usage

```rust
use cherry_pit_gateway::MsgpackFileStore;
use cherry_pit_core::DomainEvent;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
enum OrderEvent {
    Created { name: String },
}

impl DomainEvent for OrderEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::Created { .. } => "order.created",
        }
    }
}

// Create a store pointing at a temporary directory (CHE-0038:R5).
let dir = tempfile::tempdir().unwrap();
let store = MsgpackFileStore::<OrderEvent>::new(dir.path());
```

## Design

- One `.msgpack` file per aggregate (CHE-0036:R1), rewritten on every append (CHE-0036:R2)
- Temp-file + fsync + rename atomic write protocol (CHE-0032)
- Advisory file lock for single-process ownership (CHE-0043, CHE-0006:R1)
- Per-aggregate write locks via `scc::HashMap` (CHE-0035:R2); lock-free reads (CHE-0035:R3)
- Sequential ID assignment under global mutex (CHE-0035:R1)

## Operational Recovery

See [RUNBOOKS.md](RUNBOOKS.md) for operator procedures:

- Orphan temp-file recovery (CHE-0047:R1)
- Corrupt data classification (CHE-0047:R2)
- Stream quarantine (CHE-0047:R3)
- Dead-letter schema (CHE-0047:R4)
- Stale lock recovery (CHE-0047:R5)
- Migration recovery (CHE-0047:R6)

## Status

v0.1.0 — `MsgpackFileStore` is the sole event store implementation.
`object_store`-backed stores are planned for a future release (CHE-0044).

Part of the [cherry-pit](../../README.md) workspace.
