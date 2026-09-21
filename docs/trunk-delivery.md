# Canonical library delivery through the proving consumer

This is the authoritative cross-repository operational doctrine for
`Mattilsynet/gh-report`, `acje/cherry-pit`, and `acje/pardosa`, recorded from user
authority in `ghr-hxyqs.66` (2026-09-20). Each repository retains authority over
its own source, specifications, ADRs, and verification gates. This document
references the fleet engineering priorities rather than defining another list.

## Product and architectural boundary

Cherry and Pardosa are reusable products; gh-report is their proving consumer.
Keep reusable ports minimal and justified by actual consumer requirements.
Neutral Cherry normal/build dependencies remain independent of Pardosa. The
outer persistent projection adapter is hosted in Cherry under its accepted
`CPP-0001`; the application does not currently link that adapter. Pardosa core
remains independent. Consumer-specific GitHub, organization, credential, report,
and tenant policy belongs in gh-report. The completed one-way Cherry
consolidation is not a migration to repeat or backport to the obsolete tree.

Prefer standard Rust, Cargo, Clippy, and GitHub tooling. Retain bespoke checks
only for demonstrated semantic gaps; compare actual compiler-version behavior,
false positives, false negatives, and fixtures before replacing a gate. Keep
required CI contexts and plant → fail → revert → clean evidence for guard edits.

The Rust 1.98 audit (`ghr-nabkn`) established no generic standard-tool parity
for RST-0006's declaration predicate; retain `non-exhaustive-check`.
The approved cfg repair (`ghr-9l9pp`) evaluates `test=false`, treats other
options as unknown, and skips only completely parsed false predicates.
This remains a syntax checker: cfg_attr/macro expansion, out-of-line ancestry,
derive identity and aliases are outside its coverage. No universal
zero-overflag or standard-lint equivalence is claimed.

Beta permits breaking APIs and fresh schema 24; preserving old beta data or
history formats is not an acceptance obligation. Git history and unresolved
donor work remain preserved. Model committed, uncommitted, and indeterminate
outcomes explicitly. An error is not absence; cancellation is not rollback.
Do not introduce string-based recovery, fictitious atomicity/exactly-once
guarantees, or brokers without a concrete requirement. New public contracts,
material hosting changes, and resource budgets require an explicit decision.

## Trunk release boundary

Work in the existing canonical `main` checkouts. Integrate small, coherent,
reviewed increments, keeping the trunk deployable. Whole-range review includes
every unpublished commit plus working changes; a narrow historical approval
cannot approve a larger batch. Reconcile coupled history as a coherent batch
when necessary rather than dropping or parking commits.

Dependency execution follows the effective approved intake policy, including
actual build-script/proc-macro reads and compiler/archive provenance where
required. Record lock identity, source revision, environment, and command exits.
Run the applicable repository verification tiers and obtain independent review
before source publication. Producer verification does not execute consumer
tests, and consumer verification does not execute git dependencies' test targets.

Use reviewed, green, exact-head PRs. The user-authorized admin merge mechanism
must still name the reviewed head and preserve required checks/reviews; it is
not permission to bypass protections. Bind the authorized merge with
`gh pr merge <pr> --admin --match-head-commit <reviewed-head>` and the approved
merge method. Minimal ephemeral remote PR heads are
allowed, with safe deletion after merge. Local branch checkout, new worktrees,
clones, stash, archives, history resets, force pushes, hook bypasses, protection
changes, and permission workarounds are outside this workflow.

After each coherent producer cycle, adopt exact canonical `main` revisions and
lock resolution in the consumers, then deploy gh-report from its committed
`main`. A producer-only shipment is an intermediate checkpoint. Immediately
before final acceptance, reread GitHub API heads and verify exact pinned-main
agreement for the producer chain. Reconcile head races at most twice; a further
race is a reported blocker, not an unbounded retry or stale-head acceptance.

This authoritative document ships in the same consumer commit/PR as the
adoption. It is not disposable local planning output.

## Deployed consumer proof

Use localhost port 18080, MaxRepos 20, persistent NATS port 24222 and monitoring
port 18222. Record secret-safe runtime cwd, PID, executable identity, canonical
main SHAs, lock/build record, and actual binary hash. Observe timestamped logs
for an actual 60 seconds after deployment and verify behavior, persistence, and
freshness. Remediate failures before the next producer cycle.

Preserve the running application's data and avoid unnecessary NATS restarts or
resets. Fresh schema 24 is authorized only on the exact disposable local NATS
instance when needed; it is not general data-destruction authority. A failed
review, intake, or check stops rollout. Correct published work with corrective
commits/PRs rather than rewriting history.

Final acceptance requires all three canonical `main` checkouts clean and
synchronized, exact committed producer pins, and deployed consumer proof.
Retain Cargo build artifacts. Preserve per-repository Beads audit evidence and
unrelated work; explicitly reconcile metadata rather than using blanket ignores
to manufacture a clean status. Architectural roadmap owners remain open until
their own acceptance evidence exists.
