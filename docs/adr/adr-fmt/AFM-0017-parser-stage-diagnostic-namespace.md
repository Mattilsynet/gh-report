# AFM-0017. Parser-Stage Diagnostic Namespace

Date: 2026-04-29
Last-reviewed: 2026-04-29
Tier: B
Status: Accepted

## Related

References: AFM-0003, AFM-0006, GND-0005

## Context

`adr-fmt` previously swallowed parser-stage failures: a file
matching the prefix filename pattern but failing to parse
(unreadable, or missing/malformed `# PREFIX-NNNN. Title`) dropped
silently from the corpus. The user saw a clean lint while the ADR
was invisible to rule checks. AFM-0003 partitions findings into
advisory warnings (rules) and infrastructure errors (`exit 1` on
stderr). Parser-stage problems are neither: per-file content
failures the user must fix, but a single malformed file should
not abort the whole lint. They belong in the same diagnostic
stream as rule findings, with their own namespace.

## Decision

Surface parser-stage failures as advisory diagnostics in a dedicated
`P###` rule-code namespace, merged with rule diagnostics in the
unified `--lint` output stream.

R1 [5]: Define parser-stage diagnostic codes in the `P###`
  namespace, distinct from `T###` (template), `L###` (links),
  and `I###` (integrity), to signal that the file failed to
  parse rather than violated a rule
R2 [5]: Emit `P001` via `Diagnostic::warning("P001", path, 0, msg)`
  in `crates/adr-fmt/src/parser.rs` when a file matches the
  domain prefix filename pattern but `fs::read_to_string` fails;
  the ADR is excluded from rule checks for that run
R3 [5]: Emit `P002` via `Diagnostic::warning("P002", path, 0, msg)`
  in `crates/adr-fmt/src/parser.rs` when a file is readable but
  contains no `# PREFIX-NNNN. Title` H1 header recognized by
  `parse_title`; the ADR is excluded from rule checks for that run
R4 [5]: Return `Result<ParseOutcome, String>` from `parse_domain`
  and `parse_stale`; reserve the `Err` arm for unreadable directory
  entries that route through the AFM-0003 R1 infrastructure-error
  channel via `eprintln!` plus `process::exit(1)`
R5 [6]: Merge parser diagnostics with rule diagnostics in
  `crates/adr-fmt/src/main.rs` before calling
  `output::render_diagnostics` so the AFM-0003 R3 stdout
  contract surfaces a single combined `## Diagnostics` block

## Consequences

Malformed ADRs no longer vanish from the corpus silently — users
see exactly which files failed to parse and why. Rule diagnostics
and parser diagnostics share one stream and one exit-code contract.
Per-entry `io::Error` from `fs::read_dir` iteration (file vanished
mid-iter) remains a silent skip since it is a transient race rather
than a user-fixable content issue. New parser-stage codes follow
`P###` ordering; reserve `P001`–`P099` for parser-stage diagnostics.
