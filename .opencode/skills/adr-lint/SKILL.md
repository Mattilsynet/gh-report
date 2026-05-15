---
name: adr-lint
description: Run `adr-fmt --lint` against the ADR corpus and triage diagnostics by namespace (T0xx template, L0xx links, S0xx supersedes/lifecycle, P0xx parser). Use when the user asks to lint ADRs, check ADR integrity, validate the corpus, or mentions "adr-fmt --lint".
---

# adr-lint

Wrapper skill around `adr-fmt --lint`. Read-only against the corpus
(AFM-0001). Advisory exit semantics: warnings ≠ failure (AFM-0003).

## Run

Build is cheap and dogfoods the latest source. Prefer the freshly-built
binary over a stale `target/release/adr-fmt`:

```bash
cargo run -p adr-fmt --release -- --lint
```

Capture both stdout and the exit code. Do not collapse them.

| Exit | Meaning | Action |
|------|---------|--------|
| `0`  | Analysis complete (warnings allowed) | Triage, do not "fail" the run |
| `1`  | Infra error or structural error | Halt; report verbatim |

Never edit the corpus to silence a warning unless the user explicitly
asks for the fix. Warnings are signal for humans, not errors to
suppress.

## Diagnostic namespaces

| Prefix | Source | Typical content |
|--------|--------|-----------------|
| `T0xx` | `rules/template.rs` | Per-file MADR template conformance |
| `L0xx` | `rules/links.rs`    | Cross-file link integrity |
| `S0xx` | `rules/links.rs`    | Lifecycle / Supersedes consistency |
| `P0xx` | `parser.rs` (AFM-0017) | Parser-stage structural diagnostics |

When reporting back to the user, group findings by prefix and cite the
ADR ID + file:line as printed by adr-fmt. Do not paraphrase the
diagnostic message — quote it.

## Reply shape

Terse, parseable. Skip prose.

```
Lint: <PASS_CLEAN | PASS_WITH_WARNINGS | FAIL>
Exit: <n>
ADRs scanned: <n>
Diagnostics: T=<n>  L=<n>  S=<n>  P=<n>  other=<n>
Top findings:
  - <code> <ADR-ID> <file>:<line> <message>
  - ...
Notes: <abnormal stderr lines, if any>
```

`PASS_CLEAN` only when diagnostic count is 0. Any warnings → `PASS_WITH_WARNINGS`.
`FAIL` only on exit ≠ 0.

## Triage heuristics

- **L016 ("parent tier is weaker leverage than child — heuristic, may be intentional")**
  is intentionally noisy advice; surface count but do not recommend
  fixes unless the user is auditing tier hygiene.
- **P0xx** indicates a malformed ADR file — usually the highest-signal
  finding; surface first regardless of count.
- **T0xx** clusters often share a root cause (e.g. one missing section
  template); group by message before listing.

## Discipline

- Do not invent flags. The CLI surface is frozen for v0.1; only
  `--lint` is in scope here.
- Do not run from a subdirectory expecting a different corpus —
  discovery walks up to `adr-fmt.toml` (AFM-0001); there is no
  `--corpus` override.
- Do not fabricate counts. If parsing the output is ambiguous, paste
  the raw tail and say so.
