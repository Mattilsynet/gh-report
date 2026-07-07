# Solon

Enabling constraints for agent-first development

Cherry-pit is a library family primarily for coding agents. It constrains compositions that could quietly break how the system fits together. The rules it enforces live in a sizable ADR corpus of ~190 active documents under docs/adr/, each averaging about 5 rules.

A Rust workspace (edition 2024, MSRV 1.96) shipping binaries
(`adr-fmt`, `adr-srv`, `gh-report`, `comment-free`) plus their
supporting library crates and a governed ADR corpus.

## What's here

- **`adr-fmt`** — read-only ADR template and link-integrity validator.
  See [`crates/adr-fmt/`](crates/adr-fmt/).
- **`adr-srv`** — GraphQL service over a projection of the ADR corpus.
  See [`crates/adr-srv/`](crates/adr-srv/).
- **`comment-free`** — doc-lint tool enforcing the fleet-wide
  no-`//`-comments rule on Rust source.
  See [`crates/comment-free/`](crates/comment-free/).
- **`gh-report`** — GitHub organisation evidence collector and HTML
  reporter. Built on a family of internal `cherry-pit-*` crates
  providing an event-sourcing substrate (core, gateway, projection,
  agent, web, work-queue, storage primitives), with durable events
  persisted through the `pardosa*` `.pgno` store (or a NATS/JetStream
  backend). The served dashboard reports per-repository security
  posture, per-team ownership and orphaned repositories, and inline
  remediation guidance. See [`crates/gh-report/`](crates/gh-report/).
- **ADR corpus** at [`docs/adr/`](docs/adr/). Two domains are actively
  edited: `adr-fmt/` (prefix `AFM`) governs the validator; `cherry/`
  (prefix `CHE`) governs cherry-pit, adr-srv, and gh-report.
  Foundation domains (`ground`, `common`, `rust`, `security`, `flow`)
  supply cross-cutting principles applied to all crates.

## Quickstart — adr-fmt

`adr-fmt` discovers its corpus via `adr-fmt.toml` at the workspace root.

```console
cargo build -p adr-fmt
cargo test  -p adr-fmt
cargo run   -p adr-fmt -- --lint
cargo run   -p adr-fmt -- --tree CHE
cargo run   -p adr-fmt -- --refs CHE-0054
cargo run   -p adr-fmt -- --context cherry-pit-core
```

Full rule taxonomy (T0xx template, L0xx links, S0xx lifecycle, P0xx
parser) is in [`crates/adr-fmt/README.md`](crates/adr-fmt/README.md).

## Quickstart — gh-report

`gh-report` runs as a daemon (or one-shot, for baseline inspection).
It polls a GitHub organisation, persists evidence as pardosa events to
a local embedded `.pgno` event store (default; a NATS/JetStream backend
is also selectable), and serves an HTML report. There is **no offline /
fixture mode** — the binary always reaches the GitHub
API. Credentials resolve in this order: GitHub App, `GITHUB_TOKEN` env,
then `gh auth token` as a local-developer fallback (so a logged-in
[`gh` CLI](https://cli.github.com/) is sufficient for local runs). See
[`crates/gh-report/OPERATIONS.md`](crates/gh-report/OPERATIONS.md) for
production auth setup.

```console
cargo build -p gh-report --release

# Daemon mode (collects from GitHub; persists to ./store/; serves HTML)
cargo run -p gh-report -- --org <your-org> --store-dir ./store

# Inspect the persisted baseline (replays ./store/events/<org>/; writes JSON to stdout)
cargo run -p gh-report -- --dump-baseline --org <your-org> --store-dir ./store
```

Operational recovery procedures live at
[`crates/cherry-pit-gateway/RUNBOOKS.md`](crates/cherry-pit-gateway/RUNBOOKS.md).

## More

- Per-crate `README.md` files under [`crates/`](crates/).

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or
  <http://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or
  <http://opensource.org/licenses/MIT>)

at your option.

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the work by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms
or conditions.
