# ADR Template — Golden Reference

Last-updated: 2026-05-16

Two audiences: **Humans** (Context, Related, Consequences — narrative rationale and trade-offs)
and **Agents** (tagged rules in Decision — extracted verbatim by `adr-fmt --context <CRATE>`).
Write Context and Consequences for people; write tagged rules for machines.

---

## Template

````markdown
# PREFIX-NNNN. Title

Date: YYYY-MM-DD
Last-reviewed: YYYY-MM-DD
Tier: S|A|B|C|D
Status: Draft | Proposed | Accepted | Rejected | Deprecated | Superseded by PREFIX-NNNN
Crates: crate-a, crate-b (optional; comma-separated list scoping rules to specific crates)
Parent-cross-domain: PREFIX-NNNN — reason (optional; only when first References target is in another domain)

## Related

References: PREFIX-NNNN, PREFIX-NNNN | Supersedes: PREFIX-NNNN

## Context

[Problem statement and motivation — 7–100 words of prose, excluding code blocks.]

## Decision

[1–3 sentence summary of the chosen approach.]

R1 [N]: [Tagged rule — 7–60 words, positive imperative, unconditional]
R2 [N]: [Tagged rule — 7–60 words, positive imperative, unconditional]

## Consequences

+ becomes easier: [what improves]
− becomes harder: [what becomes harder or breaks]
risks/migration: [breaking changes, migration scope, open questions]
````

The structured `+` / `−` / `risks/migration` form is **preferred**.
Free prose is also accepted; no lint enforces the schema.

---

## Field Reference

### Title

`# PREFIX-NNNN. Title` — H1, domain prefix + zero-padded 4-digit number + dot + title.
Prefix must match `adr-fmt.toml`; number must match filename. Title names the *decision*, not the problem.
**Enforced by:** T001, N001–N004.

### Date / Last-reviewed

`Date: YYYY-MM-DD` — date first drafted; never updated. **Enforced by:** T002.

`Last-reviewed: YYYY-MM-DD` — most recent date the ADR's continued validity was confirmed.
Update on every review even if no changes are made. Research shows 20–25% of architectural
decisions go stale within two months — this field is the staleness signal. **Enforced by:** T003.

### Tier

`Tier: A` — architectural significance from Donella Meadows' twelve leverage points.
Sort order in `--context`: S first, D last. Classify by what the decision *is*, not blast radius.
First-yes-wins starting at S. See [AFM-0011](adr-fmt/AFM-0011-meadows-aligned-tier-classification.md).

| Tier | System characteristic | Meadows levels | Classification question |
|------|----------------------|----------------|------------------------|
| S | **Intent** — paradigm, goals, governance | 1–3 | Does this decision define the system's paradigm, system-wide architectural pattern, or decision governance? |
| A | **Self-organization** — capacity to evolve structure | 4 | Does this decision introduce or remove trait definitions, generic type parameters, or plugin boundaries that enable new implementations? |
| B | **Design** — rules, information flows | 5–6 | Does this decision prescribe a structural rule or establish an information flow — a type contract, API boundary, visibility constraint, enforcement gate, or observability requirement? |
| C | **Feedbacks** — reinforcing and balancing loops | 7–8 | Does this decision define how components observe, notify, retry, or react to each other at runtime? |
| D | **Parameters** — constants, stocks, flows, delays | 9–12 | Is this only a crate-internal implementation detail or tooling configuration value? |

**Enforced by:** T004.

### Crates (optional)

`Crates: cherry-pit-core, cherry-pit-gateway` — comma-separated crates this ADR applies to.
Used by `--context` to filter rules per crate. Omit for domain-wide decisions. ADRs without
`Crates:` are still included; only ADRs with a non-matching list are excluded.

### Parent-cross-domain (optional)

`Parent-cross-domain: COM-0018 — reason` — suppresses **L011** when the first `References:`
target is in a different domain. Value: target ADR ID + em-dash + reason. Suppression matches
**exactly** (field ID must equal first `References:` target). Per AFM-0020, every non-Root
ADR's structural parent is its first `References:` target; this field is the explicit
acknowledgment that a cross-domain parent is intentional.
**Suppresses:** L011. **Enforced by:** L018 (declared ID must match first References target),
L019 (declared target must exist in the corpus).

### Status

`Status: Accepted` — lifecycle state in the preamble (before any H2 heading).
`Amended` is not a valid status.

| State | Meaning |
|-------|---------|
| Draft | Under development, not yet proposed for review |
| Proposed | Submitted for review, awaiting acceptance |
| Accepted | Active — rules extracted by `--context` |
| Rejected | Evaluated and declined — requires Retirement section |
| Deprecated | Was accepted, no longer recommended — requires Retirement |
| Superseded by PREFIX-NNNN | Replaced by another ADR — requires Retirement |

Only `Accepted` ADRs have rules extracted. Terminal states require `stale/` move + `## Retirement`.
**Enforced by:** T005, T006, S004–S006.

### Related

`References: PREFIX-NNNN, PREFIX-NNNN | Supersedes: PREFIX-NNNN` — pipe-separated relationships
on one line inside `## Related`. References count is tier-scaled (T020). Three permitted verbs:

| Verb | Meaning | Use when |
|------|---------|----------|
| Root | Self-reference marking a tree root | This ADR is the root of a decision subtree |
| References | Soft citation | This ADR cites another for context or builds on it |
| Supersedes | Replaces target entirely | This ADR obsoletes a previous decision |

**First References target is the structural parent** (AFM-0020). List the most specialized
applicable parent first; foundation citations follow. Cross-domain first-citation triggers **L011**
unless suppressed via `Parent-cross-domain:`. `Root` + `References` cannot coexist (L009).
Every ADR needs at least one relationship (T007).
**Invalidation test:** if removing the first target makes the ADR collapse, ordering is correct.
**Enforced by:** T007, T020, L001, L003, L007–L019.

### Context

Humans only — agents do not see this section. Explain *why* the decision was needed:
problem statement (1–3 sentences), alternatives evaluated with pros/cons, and selection
rationale. Document rejected alternatives — otherwise agents re-propose them.
Code blocks encouraged; prose 7–100 words. **Enforced by:** T008, T015, T014.

### Decision

Dual audience — prose summary for humans; tagged rules for agents. Only tagged rules
extracted by `--context`.

#### Tagged Rules Format

Pattern: `RN [L]: text` — N sequential integer, L Meadows leverage layer (1–12).
Multi-line: indent continuation lines ≥2 spaces; blank line or next `RN [L]:` terminates.
Global ID rendered by `--context`: `[PREFIX-NNNN:RN:LN]` (e.g., `[CHE-0042:R1:L5]`).
Constraints: sequential IDs from R1, **tier-scaled max** (S/A/B/C/D — default B-tier limit 10),
7–60 words, layer 1–12. All statuses require tagged rules (no Draft/Proposed exemption).
Rule layers materially weaker than the ADR's tier emit **T019** (tension warning); excess
rule count emits **T016**.

**Why max 10:** P(all followed) = P(individual)^N. At 90% per-rule compliance,
10 rules → 35% all-correct; 15 rules → 21%.

**Layer table:**

| Layer | Leverage point (Meadows) | Tier |
|-------|--------------------------|------|
| 1 | The power to transcend paradigms | S |
| 2 | The mindset or paradigm out of which the system arises | S |
| 3 | The goals of the system | S |
| 4 | The power to add, change, evolve, or self-organize system structure | A |
| 5 | The rules of the system (incentives, punishments, constraints) | B |
| 6 | The structure of information flows (who does and does not have access) | B |
| 7 | The gain around driving positive feedback loops | C |
| 8 | The strength of negative feedback loops | C |
| 9 | The lengths of delays, relative to the rate of system change | D |
| 10 | The structure of material stocks and flows | D |
| 11 | The sizes of buffers and other stabilizing stocks, relative to their flows | D |
| 12 | Constants, parameters, numbers | D |

#### Prose in Decision

Prose, `###` headings, and code blocks in Decision are for humans — NOT extracted by `--context`.
Use for implementation details, code examples, and future work.
**Enforced by:** T009, T011, T014, T016, T019.

### Consequences

Preferred form (structured schema):

```
+ becomes easier: [what improves]
− becomes harder: [what becomes harder or breaks]
risks/migration: [breaking changes, migration scope, open questions]
```

Free prose also accepted; no lint enforces the schema.
Humans only — not extracted by `--context`. Document both sides: what improves and what
becomes harder. Reference affected ADRs, crates, and open questions.
**Enforced by:** T010, T015, T014.

### Retirement (terminal states only)

Required when status is Rejected, Deprecated, or Superseded. File must move to `stale/`.
Reduce to stub (AFM-0022): preamble + optional `## Related` (Supersedes edges only) +
`## Retirement`. Delete Context, Decision, Consequences, and References lines in the same
commit — git history preserves prior content.
Convention inside `## Retirement`: `Superseded-by:` / `Moved-to-stale:` / `Reason:` triple.
Narrative-only retirements also accepted.
**Enforced by:** S004, S005, S006, S007.
