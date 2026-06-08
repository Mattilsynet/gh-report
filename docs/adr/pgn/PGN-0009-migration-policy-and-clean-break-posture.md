# PGN-0009. Migration Policy and Clean-Break Posture

Date: 2026-06-08
Last-reviewed: 2026-06-08
Tier: B
Status: Accepted
Crates: pardosa

## Related

References: PGN-0001, PGN-0008

## Context

Sources rescue ADR-0019 (open-time migration policy — Accepted as a design contract; implementation gated on a follow-up mission) and rescue ADR-0017 (pre-deployment clean-break posture). PGN-0008 reserves the `EventStore::open_with_migration` slot but ships no symbol; this ADR pins the semantic contract that the future implementation mission satisfies. The clean-break posture authorises pre-publish surface breaks when they restore a doctrinal rule and bounds soft-deprecation windows to one minor cycle. Out-of-band `pardosa::store::migrate::migrate_keep` remains the only public migration path until the implementation lands.

## Decision

`MigrationPolicy` is a per-open decision (not per-event) consulted at journal open on a schema-hash mismatch. Allowed directions in v0: `Refuse`, `ShadowRead { upcast }`, `ShadowCopy { upcast, sink }`. `InPlace` is rejected unless explicitly lifted by amendment. The migration error enum is `#[non_exhaustive]` per PGN-0006. `ShadowCopy` never mutates the old sink; the new sink is either valid-on-success or unopenable-on-failure. `ShadowRead` preserves identity (`EventId`, `FiberId`); `ShadowCopy` mints fresh identity. The clean-break posture binds every pre-publish doctrinal violation as fix-now.

R1 [5]: `MigrationPolicy` is consulted once at open time, never per-event;
  per-event upcast (`ShadowRead`, `ShadowCopy`) is configured by closures
  the policy carries, not by post-open callbacks.
R2 [5]: The v0 closed direction set is `{Refuse, ShadowRead, ShadowCopy}`;
  `InPlace` is rejected and requires an explicit amendment defining its
  partial-failure and fsync/rollback contract before admission.
R3 [5]: `ShadowCopy` leaves the old sink byte-identical to its pre-open
  state; on failure the new sink is unopenable as a valid journal —
  rollback is delete-and-restart, not journaled rewind.
R4 [5]: `ShadowRead` preserves `EventId` and `FiberId` (payload-only
  upcast); `ShadowCopy` mints fresh identity per dragline-local allocation;
  cross-journal correlation lives in payload.
R5 [5]: A `LineCursor` sidecar from the old journal is reusable against a
  `ShadowRead` reopen of the same journal and rejected as a typed error
  against a `ShadowCopy` new journal; silent advance against a fresh
  `EventId` space is forbidden.
R6 [5]: Under the clean-break posture, every doctrinal violation observed
  pre-publish is fixed by the smallest patch restoring the rule exactly;
  soft-deprecation windows are bounded to one `0.x.0 → 0.(x+1).0` cycle.
R7 [4]: Substrate purity (PGN-0001), sealed-trait closure (PGN-0003 /
  PGN-0007), reader/writer split (PGN-0007), and deterministic canonical
  encoding (PGN-0003) are not subject to clean-break relaxation.

## Consequences

+ becomes easier: future `MigrationPolicy` lands against a written contract;
  cross-version recovery via reset-first or out-of-band `migrate_keep`;
  retiring soft-deprecations carried "pending publish".
− becomes harder: cross-backend live migration (out of scope; the recipe
  is a Phase 6 docs artefact, not a code seam); silently accepting a
  doctrine violation as "tolerable pre-1.0".
risks/migration: ADR-0019 is design-only; ships no Rust types. The
  closed direction set narrows future flexibility — `InPlace` and snapshot-
  shaped policies require amendments. `.pgno` byte layout (PGN-0004) and
  schema-hash derivation (PGN-0003) are unchanged.
