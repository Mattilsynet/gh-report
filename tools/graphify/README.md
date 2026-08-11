# tools/graphify/

Durable, committed records of graphify wiring that lives OUTSIDE version
control (`.git/hooks/` is never cloned, pushed, or diffed). Both files here
document machine-local edits that must be re-applied after a fresh clone or
after `graphify hook install` clobbers the hook bodies between its sentinel
markers.

## worktree-guard.sh

Guards `.git/hooks/post-commit` and `.git/hooks/post-checkout` against firing
a scoped/partial rebuild when a commit or checkout happens inside a linked
worktree (F6). See the file's own header for the exact re-apply procedure.

## post-checkout rebuild: disabled (F7)

`.git/hooks/post-checkout` no longer launches a background rebuild on branch
switch. A full rebuild on every branch switch is expensive on a 20-crate
workspace, and mission agents switch branches constantly — the cost was paid
on every switch for no corresponding benefit, since `post-commit` (scoped to
changed paths) already keeps the graph current for the normal edit/commit
cadence.

**MACHINE-LOCAL — not committed, must be re-applied after a fresh clone or
after `graphify hook install` clobbers the hook body.** The disable is a
short-circuit inserted immediately after the existing
`[ ! -d "graphify-out" ] && exit 0` guard, before the rebase/merge-skip block:

```sh
# F7: disabled. A full background rebuild on every branch switch is expensive
# on a 20-crate workspace and mission agents switch branches constantly.
# Rebuilds now happen on commit only (post-commit hook, scoped to changed
# paths). Re-enable by deleting this block; see tools/graphify/README.md
# for the re-apply procedure and rationale.
echo "[graphify] post-checkout rebuild disabled (F7) - rebuilds happen on commit only" >&2
exit 0
```

Re-apply procedure after a fresh clone or an install/clobber: open
`.git/hooks/post-checkout`, locate the `# Only run if graphify-out/ exists`
block, and insert the snippet above directly after its closing `fi`. Verify
with `sh -n .git/hooks/post-checkout` (must pass) and then confirm no new
lines appear in `~/.cache/graphify-rebuild.log` after a branch switch.

To re-enable the rebuild instead (revert this disable), delete the inserted
block; the pre-F7 hook always ran the rebuild after checking BRANCH_SWITCH
and the rebase/merge/worktree guards. If `worktree-guard.sh`'s guard block
(above) has also been clobbered, re-apply both together — `graphify hook
install` regenerates the entire sentinel-delimited hook body, so a re-run
clobbers the worktree guard, the F7 disable, AND the stub-filter tail
wiring (below) simultaneously; check all three after any `hook install`.

## filter-stubs.py

`python3 tools/graphify/filter-stubs.py [--dry-run] [--graph PATH]`

Strips per-file stdlib/external type-stub nodes (empty `source_file` AND
empty `source_location`) and their incident links from `graphify-out/graph.json`
after a rebuild. See the script's module docstring for the full mechanism,
safety rails (refuses to write on 0-node result or >40% removal), and the
explicit non-goal of recomputing community assignments.

### Hook wiring (MACHINE-LOCAL — not committed, must be re-applied)

Every rebuild launched by `.git/hooks/post-commit` regenerates
`graphify-out/graph.json` unfiltered — the stub nodes come back. The filter
must run again after each rebuild for the graph to stay clean. Because the
rebuild is detached (backgrounded, see the hook's own comments), the filter
cannot simply be appended as the next line in the hook body — it must be
chained onto the end of the backgrounded rebuild command itself so it runs
after the rebuild completes, not before.

`.git/hooks/post-checkout` no longer rebuilds at all (F7, disabled — see
above), so this tail wiring only needs to live in `post-commit` going
forward. Its `_src` heredoc still carries its own copy of the snippet below,
inserted immediately after the `_rebuild_code(...)` call succeeds — the
historical rationale (the rebuild is detached/backgrounded) is unchanged:

```python
    import subprocess as _sp
    _sp.run([sys.executable, str(Path('tools/graphify/filter-stubs.py'))], cwd=os.getcwd())
```

This re-applies on every machine independently (`.git/hooks/` is per-clone).
`graphify hook install` will clobber this edit along with the rest of the
hook body between its sentinel markers — re-apply it the same way you
re-apply `worktree-guard.sh`'s guard block after any `hook install` re-run.

Verification that the wiring works end-to-end (rebuild launches the filter,
stub count returns to 0 without manual intervention) is captured in evidence
bead `mission:graphify-improve` (sub-mission `graphify-improve-03`), not
repeated here — this file documents the mechanism, not a point-in-time proof.
