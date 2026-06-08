# adr-fmt

ADR template and link-integrity validator. A read-only analysis tool that
serves as the single source of truth for ADR governance rules across a
multi-domain ADR corpus.

## Modes

```text
adr-fmt                     # default: print governance guidelines
adr-fmt --lint              # lint all ADRs
adr-fmt --refs <ADR_ID>     # inbound references (References + Supersedes)
adr-fmt --context <CRATE>   # decision rules for a crate
adr-fmt --tree [DOMAIN]     # domain tree overview
```

The corpus location is discovered by walking up from the current
working directory until an `adr-fmt.toml` with a valid `[corpus]`
table is found. Run from anywhere inside the workspace.

Exit codes:
- `0` — analysis complete (warnings only, or clean)
- `1` — infrastructure error or lint errors detected

Warnings are advisory by design (per AFM-0003): a corpus emitting
warnings still exits `0`. Exit `1` is reserved for infrastructure
failures and structural lint errors that prevent analysis. Treat
warnings as signal for review, not as build-breakers.

## Configuration

`adr-fmt.toml` lives at the workspace root. It declares the corpus
location, domains, the stale folder, and crate-to-domain mapping.
See this repository's own `adr-fmt.toml` for a worked example.

## Bootstrap on a fresh corpus

Starting from an empty repository (no existing ADRs):

1. **Install.** `cargo install --path crates/adr-fmt` (this checkout)
   or build a release binary with `cargo build --release -p adr-fmt`
   and copy `target/release/adr-fmt` onto your `PATH`.

2. **Pick an ADR root.** Conventional choice: `docs/adr/`.

3. **Write `adr-fmt.toml`** at the repository root. Minimum viable:

   ```toml
   [corpus]
   root = "docs/adr"

   [stale]
   directory = "stale"

   [[domains]]
   prefix = "ARC"          # 2–4 uppercase letters
   name = "Architecture"
   directory = "arc"       # relative to corpus.root
   description = "Cross-cutting architectural decisions."
   crates = []
   ```

4. **Create the directories** referenced by the config:
   `mkdir -p docs/adr/arc docs/adr/stale`.

5. **Write your first ADR** as `docs/adr/arc/ARC-0001-decision-title.md`.
   Run `adr-fmt` (no flags) to print the governance reference, which
   includes the ADR template and the rule catalogue.

6. **Validate.** `adr-fmt --lint` from anywhere inside the repository
   should exit `0` once your ADR satisfies the template rules
   (`T0xx`), link rules (`L0xx`), and lifecycle rules (`S0xx`).

The tool walks up from the current directory to find `adr-fmt.toml`,
so step 6 works from any subdirectory.

## Usage

```bash
cargo run -p adr-fmt -- --lint
cargo run -p adr-fmt -- --tree
cargo run -p adr-fmt -- --refs AFM-0001
cargo run -p adr-fmt -- --context adr-fmt
```

## Build

```bash
cargo build -p adr-fmt
cargo test -p adr-fmt
```

### Cargo.lock policy

`Cargo.lock` is committed to this repository deliberately. `adr-fmt`
ships as a binary, and a committed lockfile is the standard Rust
convention for binary crates: it pins exact transitive-dependency
versions so two clones of this repository produce byte-identical
binaries given the same toolchain. Library crates typically *do not*
commit lockfiles; binary crates do.

If you depend on `adr-fmt` as a binary (via `cargo install` or a
release artefact), `Cargo.lock` is what makes that build
reproducible. If you ever consume it as a library, your own
project's `Cargo.lock` takes over and this one is ignored.

## Governance

This tool's own decisions live under `docs/adr/adr-fmt/` (prefix `AFM`).
The repository also retains a multi-domain reference corpus
(`cherry`, `common`, `flow`, `ground`, `rust`, `security`)
used to validate the tool against non-trivial real-world ADR sets.

Cross-domain governance documents:

- `docs/adr/TEMPLATE.md` — ADR template
- `FOCUS.md` — current schwerpunkt
- `adr-fmt.toml` — workspace-root configuration consumed by this tool

## License

Licensed under either of:

- Apache License, Version 2.0
- MIT license

at your option (SPDX: `Apache-2.0 OR MIT`). Contributions are
accepted under the same dual licence by default.
