# gh-report

GitHub organisation-evidence collector and HTML reporter. `gh-report` polls a
GitHub organisation and serves a dashboard covering per-repository security
posture, per-team ownership, orphaned repositories, and inline remediation
guidance.

## Quickstart — gh-report

`gh-report` runs as a daemon (or one-shot, for baseline inspection). It polls
a GitHub organisation, persists evidence as pardosa events to a local
embedded `.pgno` event store (default; a NATS/JetStream backend is also
selectable), and serves an HTML report. There is **no offline / fixture
mode** — the binary always reaches the GitHub API. Credentials resolve in
this order: GitHub App, `GITHUB_TOKEN` env, then `gh auth token` as a
local-developer fallback (so a logged-in [`gh` CLI](https://cli.github.com/)
is sufficient for local runs). See
[`crates/gh-report/OPERATIONS.md`](crates/gh-report/OPERATIONS.md) for
production auth setup, and [`crates/gh-report/README.md`](crates/gh-report/README.md)
for the crate's architecture pointers.

```console
cargo build -p gh-report --release

# Daemon mode (collects from GitHub; persists to ./store/; serves HTML)
cargo run -p gh-report -- --org <your-org> --store-dir ./store

# Inspect the persisted baseline (replays ./store/events/<org>/; writes JSON to stdout)
cargo run -p gh-report -- --dump-baseline --org <your-org> --store-dir ./store
```

Operational recovery procedures live at
[`crates/cherry-pit-gateway/RUNBOOKS.md`](crates/cherry-pit-gateway/RUNBOOKS.md).

## Why a 25-crate workspace behind one dashboard

`gh-report` is built on a `cherry-pit-*` event-sourcing substrate (core,
gateway, projection, app, web, work-queue, storage primitives), with durable
events persisted through the `pardosa*` `.pgno` store family (or a
NATS/JetStream backend). `adr-srv` is the governance plane that keeps the
ADR corpus this workspace is built against internally consistent, together
with `adr-fmt` (consumed from canonical upstream, not a member here);
`comment-free` enforces the workspace's no-`//`-comments rule and is
likewise consumed from canonical upstream rather than built here.
A few small internal tooling binaries also live here (`architect`,
`pardosa-read`, `non-exhaustive-check`). Why the substrate is developed
here as a first-class concern, rather than only as an implementation
detail of `gh-report`, is recorded in [`AGENTS.md`](AGENTS.md) § Intent —
that is the canonical statement of product stance; this README does not
restate it.

- **`gh-report`** — the dashboard described above.
  See [`crates/gh-report/`](crates/gh-report/).
- **`cherry-pit-*`** — event-sourcing substrate `gh-report` is built on
  (core, gateway, projection, app, web, work-queue, storage).
- **`pardosa*`** — durable event-store substrate (`.pgno` embedded store,
  NATS/JetStream backend, schema, wire format).
- **`adr-fmt`** — read-only ADR template and link-integrity validator.
  Consumed from canonical upstream
  [`Mattilsynet/adr-fmt`](https://github.com/Mattilsynet/adr-fmt); not a
  member of this workspace.
- **`adr-srv`** — GraphQL service over a projection of the ADR corpus.
  See [`crates/adr-srv/`](crates/adr-srv/).
- **`comment-free`** — doc-lint tool enforcing the fleet-wide
  no-`//`-comments rule on Rust source. Consumed from canonical upstream
  [`acje/comment-free`](https://github.com/acje/comment-free) as an
  installed binary; not a member of this workspace.
- **ADR corpus** at [`docs/adr/`](docs/adr/). Two domains are actively
  edited: `adr-fmt/` (prefix `AFM`) governs the validator; `cherry/`
  (prefix `CHE`) governs cherry-pit, adr-srv, and gh-report.
  Foundation domains (`ground`, `common`, `rust`, `security`, `flow`)
  supply cross-cutting principles applied to all crates.

This is a Rust workspace (edition 2024, MSRV 1.98).

## Quickstart — adr-fmt

`adr-fmt` discovers its corpus via `adr-fmt.toml` at the workspace root.
It is not a member of this workspace — it is consumed from canonical
upstream. Install the pinned revision once:

```console
cargo install --git https://github.com/Mattilsynet/adr-fmt --locked \
  --rev d27f8d4c2a02b2ff77f156783cc311ebfc081147 adr-fmt
```

Then run it against this corpus:

```console
adr-fmt --lint
adr-fmt --tree CHE
adr-fmt --refs CHE-0054
adr-fmt --context cherry-pit-core
```

Full rule taxonomy (T0xx template, L0xx links, S0xx lifecycle, P0xx
parser) is in
[`Mattilsynet/adr-fmt`](https://github.com/Mattilsynet/adr-fmt#readme).

## More

- Per-crate `README.md` files under [`crates/`](crates/).

## Contact

Owned by [`@Mattilsynet/stabsec`](.github/CODEOWNERS). For security reports,
see [`SECURITY.md`](SECURITY.md) or contact `24.7@mattilsynet.no`.

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
