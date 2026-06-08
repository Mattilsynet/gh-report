# comment-free

Doc-comment linter and non-doc-comment stripper for Rust crates.

Default mode walks `ROOT` for `.rs` files and reports doc comments whose prose
word count exceeds a budget. Fenced code blocks (` ``` ` or `~~~`) are excluded
from the count; the doctrine allows 0-3 such fenced examples per doc comment
and they do not consume the word budget. Examples are detected mechanically by
fence delimiters only — there is no semantic example detection.

Two rewrite modes are available:

- **`--rewrite` (legacy full pipeline)** — reformats files via
  `syn` → `prettyplease` → `rustfmt`; non-doc `//` and `/* */`
  comments are discarded as a side-effect of the AST round-trip.
  Doc-comment text is preserved verbatim. Surface syntax may
  normalise; whole-file rustfmt reformat is applied.
- **`--rewrite --rustdoc-link-idioms` (byte-preserving safe subpath)** —
  rewrites only `///`, `//!`, `#[doc = "..."]`, `#![doc = "..."]`, and
  `#[cfg_attr(_, doc = "...")]` payload text. Every other byte in the source
  is preserved verbatim. Does NOT run `prettyplease` or `rustfmt`, does NOT
  strip non-doc comments, does NOT touch block doc comments (`/** */`).
  Safe to dogfood.

Traversal is restricted to `.rs` files under `crates/` and `src/`. `target/`
and dotted directories are skipped.

## Exit codes

| Code | Meaning                                                            |
|------|--------------------------------------------------------------------|
| 0    | clean (no findings, no errors)                                     |
| 1    | catastrophic / unmapped IO error                                   |
| 2    | invalid CLI arguments (clap rejection)                             |
| 3    | git state error in rewrite mode (dirty / not-a-repo / git missing) |
| 4    | doc-lint findings observed (default mode)                          |
| 5    | per-file parse/IO errors observed during processing (both modes)   |

Findings (`DOC_LINT`, `REWRITE`, `WOULD_REWRITE`, diffs) go to stdout.
Metadata (`SUMMARY`, `DOC_WARN`, errors) goes to stderr.

## CLI invocations

### Default: doc-comment lint

Read-only. Walks `ROOT` (default `.`) and emits one `DOC_LINT` line per
over-budget doc comment, followed by a doctrine `DOC_LINT_MSG`. Exits 4 on
any finding.

The doctrine allows 0-3 fenced code examples (` ``` ` or `~~~` blocks) per
doc comment; their content does not count toward `--doc-max-words`. Examples
are detected mechanically by fence delimiters only — there is no semantic
example detection, and no rewriting of example content. Rendering, links, and
formatting of example bodies remain the responsibility of `rustdoc`,
`clippy`, and `rustfmt`.

```sh
comment-free
comment-free crates/foo
comment-free --doc-max-words 120
comment-free --doc-max-words 60 crates/foo
```

### Rewrite: strip non-doc comments and reformat (legacy)

Runs the legacy AST + rustfmt pipeline: `syn` → `prettyplease` →
`rustfmt`. Non-doc `//` and `/* */` comments are removed as a side-effect
of the AST round-trip; the whole file is reformatted to rustfmt's
canonical style. Doc-comment **text** is preserved verbatim.

Requires a clean git working tree under `ROOT` unless `--force-dirty` is
passed. Emits one `REWRITE` line per modified file.

```sh
comment-free --rewrite
comment-free --rewrite crates/foo
```

### Rewrite preview (dry-run)

Prints `WOULD_REWRITE` plus a unified diff per file; writes nothing. Safe on
a dirty tree.

```sh
comment-free --rewrite --dry-run
comment-free --rewrite --dry-run --context 5
comment-free --rewrite -n crates/foo
```

### Rewrite a dirty tree

Bypasses the clean-tree check. Use with care.

```sh
comment-free --rewrite --force-dirty
```

### Rewrite rustdoc-link idioms (byte-preserving safe subpath)

Mechanically normalises a small set of safe Rust intra-doc link forms.
Disabled by default; requires `--rewrite`.

When this flag is set, `--rewrite` dispatches to a **byte-preserving doc-only
subpath**: only `///`, `//!`, `#[doc = "..."]`, `#![doc = "..."]`, and
`#[cfg_attr(_, doc = "...")]` payload text is mutated. Every other byte in the
source is preserved verbatim. The full AST/rustfmt pipeline is NOT run; non-doc
`//` and `/* */` comments are NOT stripped; marker restoration is NOT invoked;
block doc comments (`/** */`) are left untouched.

```sh
comment-free --rewrite --rustdoc-link-idioms
comment-free --rewrite --dry-run --rustdoc-link-idioms crates/foo
```

Rewrites applied (only when the label is a conservative Rust item token
— `CamelCase`, `snake_case`, path-with-`::`, or `Self` / `self` /
`super` / `crate`):

| Before                       | After                              |
|------------------------------|------------------------------------|
| `[Type](Type)`               | `` [`Type`] ``                     |
| `[foo::Bar](foo::Bar)`       | `` [`foo::Bar`] ``                 |
| `[Type]` (when code-ish)     | `` [`Type`] ``                     |
| `[begin](Self::begin)`       | `` [`begin`](Self::begin) ``       |
| `[Reader](crate::Reader)`    | `` [`Reader`](crate::Reader) ``    |

Skipped (left verbatim):

- Lines inside fenced code blocks (` ``` ` or `~~~`)
- Spans inside inline code (`` `Type` ``)
- URL targets (`https://…`, `/…`, `#…`, `mailto:…`)
- Reference-style links (`[Type][ref]`) and their definitions
  (`[ref]: …`)
- Prose labels with whitespace or non-ident syntax
  (e.g. `[the writer](Writer)`)
- Targets with generics, disambiguators, or fragments
  (`Vec<u8>`, `foo()`, `m!`, `struct@Type`, `Type#variant`)
- Labels already wrapped in backticks (idempotent)
- Block doc comments (`/** */`) — the entire attribute is left verbatim
  on the safe subpath

The transform is line-count-preserving and reapplies cleanly
(idempotent). Non-doc bytes are guaranteed unchanged on this subpath.

### Override the Rust edition

Passed through to `rustfmt --edition`. Default `2024`. Has no effect when
`--rustdoc-link-idioms` is also passed (the safe subpath does not invoke
rustfmt).

```sh
comment-free --rewrite --edition 2021
comment-free --rewrite --dry-run --edition 2021 crates/legacy
```

## Flag matrix

| Flag                      | Mode      | Default | Notes                                              |
|---------------------------|-----------|---------|----------------------------------------------------|
| `ROOT` (positional)       | both      | `.`     | Must be a directory                                |
| `--doc-max-words N`       | lint      | `80`    | Prose word budget; fenced code (``` or ~~~) excluded, 0-3 fenced examples allowed |
| `--rewrite`               | rewrite   | off     | Switches to rewrite mode                           |
| `--dry-run` / `-n`        | rewrite   | off     | Requires `--rewrite`; prints diffs, writes nothing |
| `--force-dirty`           | rewrite   | off     | Requires `--rewrite`; bypasses clean-tree check    |
| `--context N`             | rewrite   | `3`     | Requires `--rewrite`; unified-diff context lines   |
| `--edition EDITION`       | rewrite   | `2024`  | Requires `--rewrite`; passed to `rustfmt` (ignored on safe subpath) |
| `--rustdoc-link-idioms`   | rewrite   | off     | Requires `--rewrite`; dispatches to byte-preserving safe doc-only subpath |

## Caveats

### `--rewrite` (legacy full pipeline)

- Reformats to `rustfmt`'s canonical style — whitespace, line-wrap, and
  attribute placement may change beyond comment removal.
- Doc-comment content is preserved, but surface syntax (`///` vs
  `#[doc = "..."]`) and whitespace may normalise. A grep for `///` may lose
  hits after a run.
- Doc-comment **text** is preserved verbatim by default; the optional
  `--rustdoc-link-idioms` flag opts in to a narrow set of mechanical link
  normalisations (see above) AND switches to the byte-preserving subpath.
- Re-injects a small set of load-bearing `//`-comment markers (currently
  the `AUTO-TRAIT-POLICY-BEGIN` / `-END` pair) around their anchor macro
  after rustfmt — see `restore_preserved_markers` in `lib.rs`.
- Requires `rustfmt` on `PATH` (`rustup component add rustfmt`). A missing or
  incompatible `rustfmt` surfaces as exit 5 with per-file `IO_ERROR`
  diagnostics.

### `--rewrite --rustdoc-link-idioms` (safe subpath)

- Mutates only doc-comment and doc-attribute payload text. All other
  source bytes — including non-doc `//` and `/* */` comments, whitespace,
  attribute placement, marker tokens inside string literals — are preserved
  verbatim.
- Block doc comments (`/** */`) are left untouched. If your file uses
  block doc comments and you want their idioms rewritten, you must use
  the legacy `--rewrite` path (which runs the full pipeline).
- Does NOT require `rustfmt` to be installed.
- Idempotent; safe to run repeatedly.
