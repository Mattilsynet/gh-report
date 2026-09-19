# cherry-pit-merger

Canonical command-side EDA primitive: a single-task merger holding the sole
`EventStore` write handle for one aggregate substrate.

Consumers implement `MergerArm`, supplying the persist-mode decision and a pure
command handler; the crate owns load, handle, create-or-append, publish, and the
I1 TOCTOU resolution. Rationale, `PersistMode` shapes, and the regression-pin
contract are governed by ADR CHE-0069.

Per CHE-0029 the crate depends only on `cherry-pit-core` and `tokio` — a sibling
of `cherry-pit-app`, not a downstream.
