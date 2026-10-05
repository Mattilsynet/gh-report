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
[`cherry-pit-gateway/RUNBOOKS.md`](https://github.com/acje/cherry-pit/blob/1ecc7b118bf5bf2ccb75c8d6a992fb65351d735e/crates/cherry-pit-gateway/RUNBOOKS.md).

## Workspace and canonical dependencies

`gh-report` is built on a `cherry-pit-*` event-sourcing substrate (core,
gateway, projection, app, web, work-queue, storage primitives), with durable
events persisted through the modern `pardosa` event-store library (version 0.5.5,
consumed from [`https://github.com/acje/pardosa`](https://github.com/acje/pardosa)
governed by the upstream [Pardosa specification](https://github.com/acje/pardosa/blob/08fcd290694553baa7fd5c3408bc18e9bd5eafb5/docs/spec/pardosa-1.0.md)). `adr-fmt` (consumed from canonical
upstream, not a member here) keeps the ADR corpus this workspace is built
against internally consistent;
`comment-free` is consumed from canonical upstream rather than built here.
Its native bounded-source doc-budget gate runs locally and in CI; the exact
installed revision, thresholds and coverage limitations live in `AGENTS.md`.
The gate is read-only and does not establish absence of non-doc comments.
Cherry is maintained in [`acje/cherry-pit`](https://github.com/acje/cherry-pit).
All eight Cherry crates and the outer test bridge use the exact git revision
declared in [`Cargo.toml`](Cargo.toml) and resolved in [`Cargo.lock`](Cargo.lock);
those files are authoritative for the current pin.
This workspace retains `gh-report`, `gh-report-web-client`, and
`non-exhaustive-check`. GitHub policy and native application stores stay here.

The former embedded library tests and fixtures live in the canonical upstream;
Cargo does not execute dependency test targets in this consumer workspace.
Historical canonical PR #5 passed all four Linux checks; that is not evidence
for the current dependency revision. Consumer verification exercises
the application and `canonical_cherry_bridge` persistence/type-identity test.

- **`gh-report`** — the dashboard described above.
  See [`crates/gh-report/`](crates/gh-report/).
- **`cherry-pit-*`** — event-sourcing substrate `gh-report` is built on
  (core, gateway, projection, app, web, work-queue, storage).
- **`pardosa`** — external durable event-store substrate (version 0.5.5,
  canonical at `https://github.com/acje/pardosa`, implementing BLAKE3
  continuous rolling commitments, 81-byte standard envelopes, and group-commit
  dragline storage).
- **`adr-fmt`** — read-only ADR template and link-integrity validator.
  Consumed from canonical upstream
  [`Mattilsynet/adr-fmt`](https://github.com/Mattilsynet/adr-fmt); not a
  member of this workspace.
- **`comment-free`** — doc-budget checker and separately authorized comment
  rewrite tool. Consumed from canonical upstream
  [`acje/comment-free`](https://github.com/acje/comment-free) as an
  installed binary; not a member of this workspace.
- **ADR corpus** at [`docs/adr/`](docs/adr/). Two domains are actively
  edited: `adr-fmt/` (prefix `AFM`) governs the validator; `cherry/`
  (prefix `CHE`) governs cherry-pit and gh-report.
  Foundation domains (`ground`, `common`, `rust`, `security`, `flow`)
  supply cross-cutting principles applied to all crates.

This is a Rust workspace (edition 2024, MSRV 1.98).

## Canonical Cherry gate ownership

The current canonical pin is owned by `Cargo.toml` and `Cargo.lock`.
Historical producer evidence for `bae8df87873c842e86113183ea892075c6debbab`
is [PR #5's completed CI run](https://github.com/acje/cherry-pit/actions/runs/35520271360)
and its [revision-qualified workflow](https://github.com/acje/cherry-pit/blob/bae8df87873c842e86113183ea892075c6debbab/.github/workflows/ci.yml).
The producer job results below refer to that historical run, not current-pin CI.

| Gate / tests | Consumer scope | Producer scope |
|---|---|---|
| Closed error enums | Locked metadata selects gh-report and all resolved Cherry library targets; missing targets or zero Cherry packages fail | Canonical library source gate |
| async-trait | All resolved `cherry-pit-*` dependency trees, including git dependencies | Canonical DAG gate |
| dead-code source suppression | Actual remaining `crates/*/src` consumer code | External libraries checked by producer `dead-code-inner-suppression-tripwire`; job 106103029927 passed |
| forbid unsafe | Four compilation roots in three local members | Producer static source gate in `build-test-lint`; job 106103030059 passed |
| Library tests and fixtures | Not executed by dependency adoption; actual `canonical_cherry_bridge` type/persistence test stays local | All transferred library tests/fixtures retained at canonical revision |

Consumer boundary on 2026-09-20: 1,699 passed, zero failed/ignored
(1,657 gh-report including doctests, 33 web-client including its doctest,
9 checker). The earlier 1,650 run lacked all-features and the other members.
Producer's previously recorded 947 tests are separate evidence, not added to
consumer counts. Browser execution remains incomplete locally until matching
wasm-bindgen 0.2.128 tooling and Chrome/chromedriver are available.

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

## Structural navigation

Graphify policy and evidence: [sole authority](../sf-sdlc/docs/graphify-evidence.md).

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
