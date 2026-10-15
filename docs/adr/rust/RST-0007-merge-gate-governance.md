# RST-0007. Merge-Gate Governance

Date: 2026-08-12
Last-reviewed: 2026-08-28 — refined — all six gates now compliant (dead-code cites RST-0003:R6) and R7's coverage claim holds as written now gate-citation rejects zero-citation gates; 2026-10-08 — gh-report scoped ruling added (history-integrity listing removal is not gate removal)
Tier: B
Status: Accepted
Parent-cross-domain: COM-0017 — merge-gate authorship and citation obligations are the general enforcement-mechanism concern COM-0017 already governs; no same-domain RST parent covers who may add a gate or what it must cite

## Related

References: COM-0017, RST-0003, CHE-0088, FLO-0014, GND-0009, AFM-0031, COM-0035

## Context

No ADR governs who may add a merge-blocking CI check, what ADR grounding
it needs, or how its YAML job id relates to the branch-protection
`name:` string. RST-0003:R4 and RST-0006:R3 instantiate CI merge gates
but state no meta-rule about adding one. Per COM-0038:R1/R4 and
AFM-0008 (domains partition by rate of change and audience), this
stays standalone rather than merging into COM-0017. `References:`
lists COM-0017 first via `Parent-cross-domain` rather than same-domain
RST-0003, deliberately overriding COM-0038:R2's same-domain-first
preference — the cross-domain parent declaration is the more specific,
load-bearing rule here. "Merge gate" is used throughout, not
"chokepoint" (CHE-0088:R2/R9) or "tripwire" (FLO-0014:R1) — both terms
are already taken.

## Decision

Authoring or amending a merge-blocking CI check requires naming the
ADR rule it enforces, cites that rule where both agents and reviewers
see it, and keeps the YAML job id — never the rendered `name:` string
— as the stable ADR-citable handle.

R1 [5]: A merge gate is a build-time, merge-blocking CI check. It is
  distinct from FLO-0014's runtime tripwire (a (warning, critical)
  pair on the FLO-0004 telemetry contract) and from CHE-0088:R2/R9's
  chokepoint (a single code-level conversion point). Existing
  *-tripwire job ids are retained; this ADR creates no rename
  obligation.
R2 [5]: Every merge gate MUST name at least one ADR rule id it
  enforces, and that rule's text MUST state the invariant the gate
  mechanizes. A gate citing a rule whose text omits the invariant is
  a defect in the gate, not the ADR. This is COM-0017:R4's inverse:
  R4 binds ADR to mechanism; R2 binds mechanism to ADR.
R3 [5]: The citation MUST appear in the gate's step `name:` and in
  the `::error::` string the gate emits on failure. It MUST NOT
  appear in the job `name:`.
R4 [6]: A workflow job's YAML id is the stable, ADR-citable identifier
  for a merge gate. A job's `name:` field renders into the
  branch-protection required-status-check context string; changing
  a `name:` is a branch-protection migration and MUST update the
  required-contexts list in the same change. ADRs cite job ids, never
  rendered `name:` strings.
R5 [5]: Adding a NEW merge gate requires the ADR it cites to carry a
  COM-0017:R4 enforcement-mechanism statement naming that gate by job
  id. Per AFM-0031:R5 this binds new and amended gates going forward;
  gates existing at this ADR's acceptance are grandfathered, with no
  flag-day migration.
R6 [5]: Removing or weakening an existing merge gate follows
  COM-0035:R5.
R7 [5]: R2's id-existence half is mechanized by the gate-citation
  check in `tools/tripwires.sh`, run as a step of the build-test-lint
  job (COM-0017:R4, GND-0009:R4). R2's invariant-match half and
  R3-R6 are not mechanically decidable; a CI rung is explicitly
  rejected for them and deferred to code review.

## Consequences

+ becomes easier: a reviewer traces any merge gate back to its
  enforced ADR rule; R2 is checked in full on every PR
− becomes harder: a new merge gate requires its ADR to carry a
  COM-0017:R4 statement naming the gate by job id up front
risks/migration: 6 merge-blocking gates, ground-truthed against
  branch protection `required_status_checks.contexts` on 2026-08-27.
  All six now comply: build-test-lint (per-step citations);
  projection-lock-tripwire (COM-0018); fence-converge-tripwire
  (CHE-0088:R10); non-exhaustive-check (RST-0006:R1+R3);
  dead-code-inner-suppression-tripwire (RST-0003:R6); supply-chain
  (SEC-0009:R7, RST-0004:R7). The last closed bd ghr-x7bm6. R7's
  coverage claim now holds as written: gate-citation additionally
  asserts every job cites at least one rule id, so no gate passes
  vacuously. R2's invariant-match half and R3-R6 stay
  code-review-tier per R7.

## gh-report scoped ruling (2026-10-08)

R6 routes "removing or weakening an existing merge gate" through COM-0035:R5's
reopen obligation. This section records the repo-scoped ruling under
COM-0035:R5 for `Mattilsynet/gh-report` main.

The branch-protection `required_status_checks.contexts` listing on main was
emptied as part of adopting history-integrity-only protection: force-push and
deletion blocked, linear history required, PR review and conversation not
required, and `enforce_admins=false` per explicit user decision (2026-10-08).
Ruled under COM-0035:R5 that re-opening is satisfied by this scoped ruling,
which records an intentional tradeoff rather than unchanged enforcement.
With `required_status_checks: null` and no effective or inherited rulesets
(verified empty via `rules/branches/main`, `rulesets?includes_parents=true`
and `rulesets`), GitHub's status-based integration blocking is removed: a
failing gate job no longer mechanically prevents a merge at the platform
level. What remains are the gate commands themselves plus process obligation:
every gate job in `ci-reusable.yml` (build-test-lint,
projection-lock-tripwire, fence-converge-tripwire, non-exhaustive-check,
dead-code-inner-suppression-tripwire, supply-chain) retains its job id,
citations, and run-and-fail behavior inside its caller workflows (`ci.yml`
on PRs to main; `build.yml` on `v*` tags); direct main pushes run neither.
Quality for direct main publication therefore rests on pre-publication
obligations — local verification tiers and gate checks, dependency intake,
and independent engineering review — not on post-push CI. For the removed
names the R4 name↔list sync obligation extinguishes with the emptied list;
if the list is reintroduced, or a retained job's `name:` changes, R4's sync
obligation resumes.

Scope: exclusively `Mattilsynet/gh-report` main. This ruling does not amend
R2/R3/R5/R7, any gate command, or any other repository. Operational policy
authority for this exception is the explicit user decision (2026-10-08);
CHE-0083/CHE-0090 supply report-side scoring and learning context, not policy
authority. The `enforce_admins=false` admin bypass is an accepted, recorded
tradeoff, not an omission: history-integrity protection is best-effort
against non-admin force-push/delete and accidents, and is not absolute
against administrators.
