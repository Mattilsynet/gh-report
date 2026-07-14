# CHE-0029. Cargo Workspace with Layered Crate DAG

Date: 2026-04-24
Last-reviewed: 2026-05-18
Tier: B
Status: Accepted

## Related

References: CHE-0074, CHE-0001, CHE-0004, COM-0014

## Context

Cherry-pit provides multiple concerns: core traits, infrastructure
adapters, serialization/transport, web serving, projections, and
agent composition. These must be organized into crates with clear
dependency boundaries.

Options:
1. **Single crate with feature flags** — simpler but couples
   everything. Users pull in dependencies they don't need.
2. **Cargo workspace (monorepo)** — each crate has a single
   responsibility. Users depend only on what they need. Clean
   dependency DAG with build parallelism.
3. **Separate repositories** — maximum isolation but coordination
   overhead for cross-crate changes.

## Decision

Cherry-pit is a Cargo workspace monorepo with 8 planned crates (2
currently active). Dependencies are shared at workspace level via
`[workspace.dependencies]`. The DAG is acyclic:

R1 [5]: Organize cherry-pit as a Cargo workspace monorepo with an
  acyclic crate dependency graph
R2 [5]: Share dependency versions at workspace level via
  [workspace.dependencies]
R3 [5]: Commit Cargo.lock to version control for reproducible
  dependency resolution across all environments
R4 [5]: Restrict cherry-pit-core/Cargo.toml [dependencies] to the
  pure-domain set serde, uuid, jiff, and any adapter-decided encoding
  dependency so the crate
  stays a leaf with zero transport, runtime, or filesystem
  dependencies; adapter crates such as cherry-pit-pardosa host substrate
  specifics per CHE-0071)
R5 [5]: Keep async runtimes (tokio), web frameworks (axum), transport
  clients (async-nats), and observability stacks (tracing) in adapter
  crates such as cherry-pit-gateway, cherry-pit-web, and pardosa
R6 [5]: Verify cherry-pit-core's transitive dependency closure in CI
  via cargo tree -p cherry-pit-core, asserting no tokio, axum,
  async-nats, or tracing crate appears in the resolved graph

```
cherry-pit-core (leaf — no cherry-pit dependencies)
├── cherry-pit-gateway
├── pardosa-genome → pardosa-derive → pardosa
├── cherry-pit-projection
└── cherry-pit-web
    └── cherry-pit-app (root — depends on everything)
```

Workspace-level configuration:
- `[workspace.dependencies]` for version consistency
- `[workspace.lints.clippy]` with pedantic warnings
- `[profile.release]` with LTO, strip, overflow-checks
- `Cargo.lock` is committed for reproducible builds

## Consequences

- Each crate has a single responsibility and minimal dependencies.
- Users depend on `cherry-pit-core` for domain work without pulling
  in infrastructure.
- Workspace-level versions prevent drift; independent crates compile
  concurrently.
- 6 planned crates are unvalidated until built; `cherry-pit-app`
  will surface cross-crate mismatches.
- `Cargo.lock` commits ensure reproducible CI and the eventual binary.
- **De-scalability invariant.** Restricting `cherry-pit-core` to
  `serde`, `uuid`, `jiff` means domain code compiles and tests run
  even if every adapter crate breaks.
- **CI enforcement closes the gap.** A `cargo tree -p cherry-pit-core`
  check makes R4 a build error rather than a convention.
