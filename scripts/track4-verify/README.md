# track4-verify

Mechanical verifier for the 12 exit criteria of the Phase 2 v2 Track 4 mission
package (`gh-report` consolidation). Collapses what would otherwise be 12+
bash calls into one deterministic invocation that emits a greppable,
parseable PASS/FAIL report.

## Usage

```
cargo run --manifest-path scripts/track4-verify/Cargo.toml
```

Run from the repo root. Exit code `0` iff all criteria PASS (MANUAL counts
as PASS); `1` if any criterion FAILs.

### Flags

- `--eventstore-ceiling N` (default 8): max allowed `EventStore` mentions
  under `crates/gh-report/src/`. Raise if Track 4.0's structural refactor
  legitimately added a few more references.
- `--strict-docs` (default off): treat doc-reconciliation heuristic miss
  as FAIL instead of MANUAL. Heuristic searches `FOCUS.md` and
  `docs/c4/roadmap.md` for the literal substring `Track 4`. If the docs
  landed but the heuristic doesn't catch it, leave `--strict-docs` off.

## Output

Tagged records (tab-separated), one per line:

```
CRITERION\t<num>\t<short_name>\t<PASS|FAIL|MANUAL>\t<metric>\t<note>
SUMMARY\t<total>\t<pass>\t<fail>\t<manual>\t<duration_ms>
```

## Caveats

1. **Rg exit-code inversion** (criterion #1): rg exits 1 on no-match, 0 on
   match. The runner inverts this.
2. **EventStore ceiling is heuristic**: criterion #2 counts matches; raise
   `--eventstore-ceiling` if Track 4.0 legitimately exceeds 8.
3. **Doc-reconciliation is heuristic**: criterion #12 substring-matches
   `Track 4` in `FOCUS.md` and `docs/c4/roadmap.md`. Default verdict on
   miss is MANUAL; use `--strict-docs` to fail hard.

## Expected pattern before Track 4.3 lands

- PASS now: #1 (smi-rename-gate), #2 (eventstore-confinement), #9 (adr-fmt-lint).
- FAIL until 4.3: #5 (LOC gate, server.rs > 2500), #12 (doc reconciliation).
- #3, #4, #6a, #6b, #7, #8 depend on whether 4.2.B's uncommitted work
  builds clean. Clippy (#7) is the likely failure mode (warnings-as-errors
  on unused items from a half-landed refactor).
