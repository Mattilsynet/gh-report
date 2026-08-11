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

## filter-stubs.py

`python3 tools/graphify/filter-stubs.py [--dry-run] [--graph PATH]`

Strips per-file stdlib/external type-stub nodes (empty `source_file` AND
empty `source_location`) and their incident links from `graphify-out/graph.json`
after a rebuild. See the script's module docstring for the full mechanism,
safety rails (refuses to write on 0-node result or >40% removal), and the
explicit non-goal of recomputing community assignments.

### Hook wiring (MACHINE-LOCAL — not committed, must be re-applied)

Every rebuild launched by `.git/hooks/post-commit` (and, until F7/sub-mission
04 disables it, `.git/hooks/post-checkout`) regenerates `graphify-out/graph.json`
unfiltered — the stub nodes come back. The filter must run again after each
rebuild for the graph to stay clean. Because the rebuild is detached
(backgrounded, see the hook's own comments), the filter cannot simply be
appended as the next line in the hook body — it must be chained onto the end
of the backgrounded rebuild command itself so it runs after the rebuild
completes, not before.

Append, inside the Python `_src` heredoc in EACH hook, immediately after the
`_rebuild_code(...)` call succeeds (both the `post-commit` and, while still
active, `post-checkout` hook bodies have their own copy of this call):

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
