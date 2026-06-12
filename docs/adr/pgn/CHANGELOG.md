# Changelog

## [Unreleased]

- breaking? N — pardosa `BackendError` adds `Connect { op, source }` and `Replay { op, source }` variants so JetStream connect/replay failures remain distinct from publish failures; adding variants to the always-`#[non_exhaustive]` enum is non-breaking per PGN-0012:R3. gh-report now carries `BackendOp` through `StoreError::BackendInfrastructure { op, source }` for operator-facing infrastructure errors. Mission bead: adr-fmt-ihppt.2.
