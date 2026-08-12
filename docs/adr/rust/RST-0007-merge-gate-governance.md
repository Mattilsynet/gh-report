# RST-0007. Merge-Gate Governance

Date: 2026-08-12
Last-reviewed: 2026-08-12 — refined — authored to correct a mis-citation (GND-0003 -> COM-0038:R1/R4) carried in the originating proposal (mission:three-threads)
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
R7 [4]: R2's id-existence half is mechanized by the gate-citation
  check in `tools/tripwires.sh`, run as a step of the build-test-lint
  job (COM-0017:R4, GND-0009:R4). R2's invariant-match half and
  R3-R6 are not mechanically decidable; a CI rung is explicitly
  rejected for them and deferred to code review.

## Consequences

+ becomes easier: a reviewer traces any merge gate back to its
  enforced ADR rule; R2's id-existence half is checked every PR
− becomes harder: a new merge gate requires its ADR to carry a
  COM-0017:R4 statement naming the gate by job id up front
risks/migration: measured debt (bd adr-fmt-yc08w) — 4 of 6 gates lack
  a COM-0017:R4 statement (projection-lock-tripwire, async-trait,
  pardosa-dep, dead-code-inner-suppression-tripwire — the last citing
  no ADR), against 2 compliant exemplars (fence-converge-tripwire ->
  CHE-0088:R10; non-exhaustive-check -> RST-0006:R3). Grandfathered
  per R5; amendment tracked in bd adr-fmt-glvkn. R2's invariant-match
  half and R3-R6 stay code-review-tier per R7 — no lint parses
  whether a cited rule's prose states the mechanized invariant.
