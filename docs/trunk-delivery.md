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

After each coherent producer cycle, adopt the nominated tested immutable
published producer cohort and its lock resolution in the consumers, then
deploy gh-report from its committed `main`. A producer-only shipment is an
intermediate checkpoint. Producers nominate verified, published revisions;
consumers keep the full family pinned at exact copies (current cohort
unchanged: `acje/pardosa` @ `e807076`, `acje/cherry-pit` @ `008d875`) with a
coherent lock, and record revision, lock, toolchain, environment, and raw
exits, reusing only exact-input proof. Producer heads may advance without
invalidating an unchanged exact-input adoption — freshness is a property of
the nominated tested cohort, distinct from latest-`main` agreement — and a
newer head becomes a later cohort, adopted on the next authorized cycle.
Local producer updates cannot enter an adoption until that publication is
authorized. Canonical family pins, intake fingerprint, and producer-native
plus consumer gates are retained; adoption consumes only committed published
revisions — never dirty, path-based, or vendored consumption — and confers no
publication authority.

This authoritative document ships in the same consumer commit/PR as the
adoption. It is not disposable local planning output.

### gh-report main history-integrity protection (scoped exception)

For `Mattilsynet/gh-report` main only, branch protection is deliberately
history-integrity-only. Force pushes and deletions are blocked and linear
history is required; mandatory PR reviews, code-owner/stale-review, and
conversation resolution are not enforced at branch-protection level, and the
required-status-checks listing is empty. Removing required statuses removes
GitHub's status-based mechanical integration blocking: with no effective or
inherited rulesets (verified empty), a failing gate no longer prevents a
merge at the platform level. This is an authorized main-only tradeoff, not
an unchanged enforcement posture. Pull requests to main remain available and
preferred, but are optional; the release workflow above does not require them
for this repository.

Administrator bypass is explicit and user-accepted: `enforce_admins=false`,
so repository admins are not bound by the branch-level history blocks.
History-integrity protection is therefore a best-effort barrier against
non-admin force-push/delete and accidents, not an absolute guarantee against
administrator action (see RST-0007 gh-report scoped ruling).

Operational authority for this exception is the explicit user decision
(2026-10-08); CHE-0083/CHE-0090 classify report-side scoring and supply
learning context only. The CI gate commands remain: every gate job in
`.github/workflows/ci-reusable.yml` (RST-0007, cited by job id) keeps its job
id and run-and-fail behavior inside its caller workflows — `ci.yml` triggers
on PRs to main, and `build.yml` triggers on `v*` tags; direct main pushes
run neither. Delivery quality for any direct main publication therefore rests
on pre-publication obligations — run the local verification tiers and gate
checks, perform dependency/supply-chain intake (cargo audit + deny), obtain
independent engineering review (mandatory linus/code-review), and retain
guard plant→fail→revert→clean evidence — with no reliance on post-push CI.
This exception is scoped to `Mattilsynet/gh-report`; it does not alter CI
triggers or add workflows, does not weaken gates or protection for any other
repository, and grants no force-push, deletion, or history-rewrite authority
anywhere.

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
