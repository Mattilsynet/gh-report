#!/bin/sh
# graphify-worktree-guard.sh
#
# Durable copy of the linked-worktree guard snippet installed in
# .git/hooks/post-commit and .git/hooks/post-checkout.
#
# WHY THIS FILE EXISTS
# .git/hooks/ is NOT version controlled — it lives outside the repo's tree
# and is never cloned, pushed, or diffed. The guard below is edited directly
# into the two hook files on each machine. This script is the durable,
# committed record of that edit so it survives:
#   - `graphify hook install`, which rewrites the hook body between the
#     `# graphify-hook-start` / `# graphify-hook-end` (and the
#     `# graphify-checkout-hook-start` / `# graphify-checkout-hook-end`)
#     sentinel markers and WILL CLOBBER this guard if it is re-run.
#   - a fresh clone or a new machine, where .git/hooks/ starts empty.
#
# THE PROBLEM (F6, reproduced 2026-08-11)
# Committing from a linked worktree (`git worktree add`) fires the same
# post-commit hook as the primary tree. The rebuild it launches is scoped to
# that worktree's checkout, which is typically partial/limited, and this
# produces a false "Rebuilt: 0 nodes" success — silently corrupting the
# shared graphify-out/graph.json state without any error.
#
# THE FIX
# `git rev-parse --git-common-dir` and `git rev-parse --git-dir` resolve to
# the SAME absolute path in the primary working tree, and to DIFFERENT
# absolute paths in any linked worktree (git-dir is a per-worktree
# subdirectory of git-common-dir, e.g. .git/worktrees/<name>). Comparing the
# two, resolved to real absolute paths, cheaply and reliably distinguishes
# "am I in a linked worktree" without depending on path names or cwd tricks.
#
# RE-APPLY PROCEDURE (after `graphify hook install` clobbers the hooks, or
# on a fresh clone/install)
#   1. Open .git/hooks/post-commit. Immediately after the existing
#      rebase/merge/cherry-pick skip block (the four `[ -d ... ] && exit 0`
#      / `[ -f ... ] && exit 0` lines checking $GIT_DIR/rebase-merge etc.)
#      and BEFORE the `[ "${GRAPHIFY_SKIP_HOOK:-0}" = "1" ] && exit 0` line,
#      paste the guard block below.
#   2. Open .git/hooks/post-checkout. Immediately after the same
#      rebase/merge/cherry-pick skip block and BEFORE the "Detect the
#      correct Python interpreter" comment, paste the same guard block
#      (only the echo prefix differs: "[graphify hook]" in post-commit,
#      "[graphify]" in post-checkout, matching each hook's existing style).
#   3. Verify: `sh -n .git/hooks/post-commit && sh -n .git/hooks/post-checkout`
#      must both exit 0.
#   4. Verify the guard does NOT fire in the primary tree: run the two
#      rev-parse commands below directly in the primary tree and confirm
#      they resolve to the same absolute path.
#
# --- guard block (identical in both hooks; echo prefix matches hook style) ---
#
# _COMMON_DIR=$(git rev-parse --git-common-dir 2>/dev/null)
# if [ -n "$_COMMON_DIR" ] && [ -n "$GIT_DIR" ]; then
#     _COMMON_ABS=$(cd "$_COMMON_DIR" 2>/dev/null && pwd -P)
#     _GITDIR_ABS=$(cd "$GIT_DIR" 2>/dev/null && pwd -P)
#     if [ -n "$_COMMON_ABS" ] && [ -n "$_GITDIR_ABS" ] && [ "$_COMMON_ABS" != "$_GITDIR_ABS" ]; then
#         echo "[graphify hook] skipping: linked worktree detected (git-common-dir != git-dir)" >&2
#         exit 0
#     fi
# fi
#
# --- end guard block ---
