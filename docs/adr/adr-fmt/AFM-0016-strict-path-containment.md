# AFM-0016. Strict Path Containment for Config-Supplied Directories

Date: 2026-04-29
Last-reviewed: 2026-04-29
Tier: B
Status: Accepted

## Related

References: AFM-0003

## Context

`adr-fmt` reads `adr-fmt.toml` and joins its `domains[].directory`
and `stale.directory` strings to the ADR root before walking the
filesystem. A malicious or buggy config could supply an absolute
path, a `..` traversal, or a symlink target that escapes the corpus,
inducing the tool to read arbitrary files when run against an
untrusted repository or pull request. Three containment strategies
were evaluated: lexical-only checks (cheap but blind to symlinks),
warn-only canonicalization (allows shared archives but weakens the
guarantee), and strict canonical containment. Strict canonical
containment was chosen because the threat surface includes CI
runners that process untrusted ADR contributions.

## Decision

Reject any config-supplied directory string that fails strict
containment, treating violations as infrastructure errors per
AFM-0003 R1.

R1 [5]: Validate every config-supplied directory through
  `containment::contained_join` or `contained_join_optional` in
  `crates/adr-fmt/src/containment.rs`, which enforce the
  full lexical-plus-canonical pipeline
R2 [5]: Reject lexically as `ContainmentError::Absolute`,
  `ContainmentError::Empty`, or `ContainmentError::ParentTraversal`
  any segment that is absolute, empty, or contains a
  `Component::ParentDir` before touching the filesystem
R3 [5]: Canonicalize the joined target via `std::fs::canonicalize`
  and verify it descends from the canonicalized ADR root via
  `Path::starts_with`; reject mismatches as
  `ContainmentError::EscapesRoot`
R4 [5]: Surface containment failures via `eprintln!` plus
  `process::exit(1)` in `crates/adr-fmt/src/main.rs` so they
  share the AFM-0003 infrastructure-error channel
R5 [5]: Canonicalize the user-supplied `cli.adr_directory` and the
  walk-up result of `resolve_adr_root_optional` so subsequent
  containment checks operate against a stable, symlink-resolved root

## Consequences

Malicious configs pointing domains at `/etc` or `../../` abort
before any read; symlink-escaping farms inside the root are also
rejected. The policy disallows shared ADR archives stitched via
out-of-tree symlinks — affected teams must vendor the files.
Canonicalization requires the target to exist, so
`contained_join_optional` returns `None` for not-yet-created
directories like a missing `stale/`. Containment is checked once
at startup; concurrent attackers on the same filesystem are out
of scope (the threat model is a malicious config, not a malicious
peer process).
