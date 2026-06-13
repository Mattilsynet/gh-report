# gh-report

GitHub organisation-evidence collector and HTML reporter built on the
**cherry-pit** event-sourcing substrate.

## Status

Alpha. The crate persists evidence to a native pardosa event store
(`crate::store::NativeStore`) over `pardosa` / `pardosa-schema` /
`pardosa-nats`, and binds a curated subset of the cherry-pit public
surface per **CHE-0073** / **CHE-0074**: `cherry-pit-wq` (work-queue,
worker-pool, budget, rate-limit), `cherry-pit-storage` (atomic writes,
`RunLock`, snapshot signatures), `cherry-pit-core` (correlation context),
and `cherry-pit-web` (projection). The default backend is an embedded
`.pgno` event store; a NATS/JetStream backend is also selectable.

## Build and run

```
cargo build -p gh-report --release
cargo run   -p gh-report -- --org <your-org> --store-dir ./store
```

Prerequisite: a logged-in [`gh` CLI](https://cli.github.com/). Credentials
resolve GitHub App → `GITHUB_TOKEN` → `gh auth token` (local fallback);
there is no fixture or offline mode. See the workspace
[`README.md`](../../README.md) for the full flag list; operational recovery
procedures are substrate-scoped at
[`crates/cherry-pit-gateway/RUNBOOKS.md`](../cherry-pit-gateway/RUNBOOKS.md).

## Documentation pointers

- **Architecture map and end-to-end trace.** See
  [`DESIGN.md`](DESIGN.md).
- **Storage remodel.** [CHE-0073](../../docs/adr/cherry/CHE-0073-gh-report-storage-remodel.md);
  native pardosa store port at
  [CHE-0074](../../docs/adr/cherry/CHE-0074-gh-report-native-pardosa-store-port.md).
- **Schema evolution.** [CHE-0022](../../docs/adr/cherry/CHE-0022-event-schema-evolution.md);
  worked example at `src/domain/events.rs:90`.
- **Correlation propagation.** [CHE-0039](../../docs/adr/cherry/CHE-0039-correlation-context-propagation.md).
- **Flat public-API discipline.** [CHE-0030](../../docs/adr/cherry/CHE-0030-flat-public-api.md).
- **Detailed design.** [`DESIGN.md`](DESIGN.md).
- **Operations.** [`OPERATIONS.md`](OPERATIONS.md).

## License

Dual-licensed under Apache-2.0 OR MIT at your option.
