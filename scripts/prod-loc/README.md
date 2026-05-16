# prod-loc

Count Rust production lines, excluding tests.

## Usage

```
prod-loc <PATH> [--details]
```

`PATH` may be a single `.rs` file or a directory walked recursively.

## Rules

- Excludes any item (and its body) annotated with `#[cfg(test)]`, or with
  a `cfg(...)` expression whose token stream contains the bare identifier
  `test` (covers `#[cfg(any(test, ...))]`, `#[cfg(all(test, ...))]`).
- Excludes every `.rs` file inside a `tests/` directory (any path
  component named exactly `tests`). A file `src/tests.rs` is NOT excluded
  by this rule — it is a file named `tests.rs`, not a `tests/` directory.
- Counts **physical lines** — blanks, comments, and doc-comments all
  count. This is LOC, not SLOC. If a future caller wants SLOC, add a
  `--sloc` flag; v1 keeps the metric simple.
- Files syn cannot parse are reported on stderr and counted as 0
  production / 0 test lines (`excluded_reason = "parse error"`).

## Output

Default (tab-separated tagged records, per AGENTS.md § Communication style):

```
PROD-LOC <total_production>
TEST-LOC <total_test>
FILES    <total_files>
```

With `--details`, one `FILE` line precedes the summary per source file:

```
FILE  <path>  <prod>  <test>  <total>  <reason-or-->
```

## Example

```
$ cargo run --manifest-path scripts/prod-loc/Cargo.toml -- \
    crates/gh-report/src/infra/server/server.rs
PROD-LOC <n>
TEST-LOC <n>
FILES    1
```

For the Track 4.3 gate, the load-bearing baseline file is
`crates/gh-report/src/infra/server/server.rs`. As of mission open the file
is ~3850 total lines, ~1000 production / ~2850 test.

## Exit codes

- `0` — measurement complete.
- `1` — path missing, no `.rs` files, or file-read error.
- `2` — invalid CLI args.

## Caveats

- The cfg-arg detector uses a word-boundary substring scan for the bare
  `test` identifier in the attribute's token stream. Robust against the
  common false-positive cases (`feature = "test"`, `target_os = "..."`).
  A hypothetical future Rust cfg key whose *bare identifier* contains
  `test` (e.g. `target_test`) would false-positive — none exists today.
- `#[cfg_attr(test, ...)]` is deliberately *not* treated as a test gate;
  it conditionally applies an attribute, not a body.
- syn parse failures degrade gracefully: stderr warning + zero counts.
  Re-run the tool isolated on any warned file to investigate.
