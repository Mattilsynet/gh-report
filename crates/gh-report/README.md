# gh-report

GitHub organisation-evidence collector and HTML reporter built on the
**cherry-pit** event-sourcing substrate.

## Status

Alpha. Authored for EVAL-GATE preparation (workspace WU-6). The crate
binds a curated subset of the cherry-pit public surface per
**CHE-0054:R8**: `Aggregate`, `DomainEvent`, `EventStore`, `EventBus`,
`AggregateId`, `EventEnvelope` from `cherry-pit-core`;
`ProjectionDriver` + `FileProjectionStore` from `cherry-pit-projection`;
`InProcessEventBus` + `ProjectionDriverExt` from `cherry-pit-app`;
and `MsgpackFileStore` from `cherry-pit-gateway`. No `App<…>` or
`CommandGateway` consumption at v0.1 (see CHE-0054:R10).

## Build and run

```
cargo build -p gh-report --release
cargo run   -p gh-report -- --org <your-org> --store-dir ./store
```

Prerequisite: a logged-in [`gh` CLI](https://cli.github.com/) — the
binary resolves credentials via `gh auth token` and has no fixture or
offline mode at v0.1. See the workspace [`README.md`](../../README.md)
§2 for the full flag list and §DoD-8 for operational recovery
procedures (substrate-scoped at
[`crates/cherry-pit-gateway/RUNBOOKS.md`](../cherry-pit-gateway/RUNBOOKS.md)).

## Documentation pointers

- **Architecture map, anti-pattern deviations, end-to-end trace.** See
  the workspace [`README.md`](../../README.md) §6.1–§6.3. The
  diff-style summary of gh-report's sanctioned CommandGateway carve-out
  (CHE-0054:R10) lives there per FOCUS.md:551–554, not in this
  crate-level README.
- **Aggregate decomposition.** [CHE-0054](../../docs/adr/cherry/CHE-0054-gh-report-aggregate-decomposition.md).
- **Schema evolution.** [CHE-0022](../../docs/adr/cherry/CHE-0022-event-schema-evolution.md);
  worked example at `src/domain/events.rs:90`.
- **Correlation propagation.** [CHE-0039](../../docs/adr/cherry/CHE-0039-correlation-context-propagation.md).
- **Flat public-API discipline.** [CHE-0030](../../docs/adr/cherry/CHE-0030-flat-public-api-private-modules.md).
- **Detailed design.** [`DESIGN.md`](DESIGN.md).
- **Operations.** [`OPERATIONS.md`](OPERATIONS.md).

## License

Dual-licensed under Apache-2.0 OR MIT at your option.
