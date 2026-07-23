# cherry-pit-gateway

Infrastructure implementations for cherry-pit port traits.

The crate's file-based, MessagePack-serialized event store was retired
per CHE-0100 (msgpack-removal-2, L2-R2). Event persistence for
cherry-pit consumers now goes through `pardosa` (`.pgno`, default
backend) or `pardosa-nats`; this crate no longer ships its own store
implementation.

## What remains

- [`StaleLockEvidence`] / [`stale_lock_evidence`] — operator-side helpers
  for the stale-lock recovery runbook (CHE-0047:R5), independent of any
  specific store backend.

## Operational Recovery

See [RUNBOOKS.md](RUNBOOKS.md) for the surviving operator procedure
(stale-lock recovery, R5). The remaining v0.1 runbooks (R1-R4, R6) were
scoped to the retired file-based store and no longer apply.

## Status

Post-msgpack-removal-2 (CHE-0100 L2-R2): no event store implementation
ships from this crate. `object_store`-backed stores remain a possible
future direction (CHE-0044) but are not implemented here.

Part of the [cherry-pit](../../README.md) workspace.
