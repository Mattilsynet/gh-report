# CHE-0043. Process-Level File Fencing

Date: 2026-04-25
Last-reviewed: 2026-04-28
Tier: D
Status: Accepted

## Related

References: CHE-0006, CHE-0032, CHE-0035, COM-0025

## Context

CHE-0006 establishes the single-writer assumption: each aggregate instance is owned by exactly one OS process. However, no enforcement mechanism existed — two processes sharing a store directory could corrupt data silently. CHE-0006 documented this gap but deferred mitigation.

## Decision

`MsgpackFileStore` acquires an exclusive advisory file lock on
`{store_dir}/.lock` before its first write operation (`create` or
`append`). The lock is:

R1 [10]: Acquire an exclusive advisory file lock on {store_dir}/.lock
  before the first write operation
R2 [10]: Lock acquisition is lazy via OnceCell, triggered on first
  write not on construction
R3 [10]: A second process on the same directory gets
  StoreError::StoreLocked instead of silent data corruption

1. **Lazy** — acquired on first write via `tokio::sync::OnceCell`,
   not on construction. Read-only operations (`load`) do not fence.
2. **Exclusive** — uses `std::fs::File::try_lock()` (Rust 1.95+
   native `flock(2)` wrapper). A second process attempting the same
   directory gets `StoreError::StoreLocked`.
3. **Process-scoped** — the `std::fs::File` handle lives in the
   `OnceCell` for the `MsgpackFileStore` lifetime. The OS releases
   the lock when the file descriptor closes (on drop).
4. **Advisory** — the lock is advisory, not mandatory. It prevents
   accidental dual-writer scenarios; it does not protect against
   malicious processes ignoring the lock.

```rust
// StoreError variant
StoreLocked { path: PathBuf }

// MsgpackFileStore field
dir_lock: tokio::sync::OnceCell<std::fs::File>

// Acquisition (inside ensure_fenced)
file.try_lock().map_err(|e| match e {
    std::fs::TryLockError::WouldBlock => StoreError::StoreLocked { path },
    std::fs::TryLockError::Error(io) => StoreError::Infrastructure(Box::new(io)),
})?;
```

No external crate is needed — Rust 1.95 stabilized `File::try_lock()`
and `File::lock()` in `std::fs`.

## Consequences

The second writer fails fast with `StoreError::StoreLocked`; read-only `load` remains unfenced. This adds no dependencies and is advisory defense-in-depth, not a security boundary. Networked, object-backed, or shared filesystems require backend-specific fencing.
