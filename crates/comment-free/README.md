# comment-free

`comment-free` is a Rust-only hygiene tool for keeping source comments small,
structured, and intentional.

Its goal is to make coding with LLM agents more efficient by reducing stale or
misleading repository context. It removes ordinary line and block comments while
preserving the two comment-shaped signals that remain load-bearing here:
`AUTO-TRAIT-POLICY-*` markers and `// SAFETY:` lines. Doc comments are kept,
normalised to idiomatic rustdoc links, and linted when they grow too long.
Repository documentation files are reported but never rewritten. Output stays
terse, structured, and informative for automated agents.

Default mode is read-only: it walks Rust source files under `crates/` and `src/`
and reports doc comments whose prose exceeds the configured word budget. Fenced
code blocks are excluded from the count, and output is tab-separated so agents
and scripts can parse it reliably.

Rewrite mode performs two byte-preserving passes outside their target text:

1. canonicalise Rust intra-doc-link idioms in doc payloads, for example
   `[Type](Type)` to ``[`Type`]``;
2. strip ordinary non-doc `//` and `/* */` comments via the rustc lexer.

Doc comments are never deleted. `// SAFETY:` lines and
`AUTO-TRAIT-POLICY-BEGIN` / `AUTO-TRAIT-POLICY-END` marker lines are preserved.
Use `--dry-run` to inspect the unified diff before writing files.

## Usage

```sh
cargo run -p comment-free -- crates/comment-free
cargo run -p comment-free -- --doc-max-words 100 crates/comment-free
cargo run -p comment-free -- --rewrite --dry-run crates/comment-free
cargo run -p comment-free -- --rewrite crates/comment-free
```

Exit codes:

- `0`: clean
- `2`: invalid CLI arguments
- `4`: doc-lint findings in default mode
- `5`: per-file parse or I/O errors during processing

`--rustdoc-link-idioms` is a deprecated alias for `--rewrite` and is retained
for one release.
