# GND-0008. Focus of Effort Is Named and Time-Boxed

Date: 2026-04-30
Last-reviewed: 2026-06-14
Tier: S
Status: Accepted

## Related

References: GND-0003, GND-0001

## Context

The corpus is uniform — every directive equally weighted on the page.
Reality is not — at any given period a small subset is load-bearing
for what the system is actively trying to achieve. The German term
*Schwerpunkt* names this focus of effort. Without it executors face
the corpus as undifferentiated checklist; with it they know which
decisions matter most *now*.

Three options:

1. **No focus signal** — every directive equally weighted always.
   Executors hedge across all of them or pick arbitrarily.
2. **Re-weight directives directly** — promote some, demote others,
   in the corpus itself. Confuses durable intent with transient
   priority.
3. **Maintain a separate, time-boxed focus pointer** — names the
   outcome, which directives are load-bearing for it, and what is
   explicitly out of focus; replaced when the period ends. Corpus
   stays intact; pointer changes.

Option 3 chosen: separates *what we believe* from *what we are
working on now*.

## Decision

A focus-of-effort artefact names the outcome the system is currently
concentrating effort on, the small set of directives load-bearing for
that outcome, and the domains explicitly out of focus for the period.
The artefact is time-boxed, replaceable without amending the corpus,
and consulted during backbriefing so executors confirm proposed action
against current focus.

R1 [3]: Maintain a single focus-of-effort artefact (e.g. bd
  `phase:`/`roadmap` labels in this repo) naming the outcome, the load-bearing
  directives by `PREFIX-NNNN` with rationale, the out-of-focus
  domains, and the period covered, refreshed each cycle
R2 [3]: Set the focus period in the artefact's `Period:` field as
  `YYYY-MM-DD..YYYY-MM-DD` and replace the artefact within seven
  days of the end date — expired focus is mis-focus
R3 [3]: Cite the current focus-of-effort artefact in every GND-0006
  backbriefing; the executor confirms the proposed action serves
  an in-focus directive by ID or names explicitly why an out-of-
  focus action is justified
R4 [3]: Record every focus-of-effort change in the focus-of-effort
  artefact rather than in any ADR — durable intent stays in
  the corpus, transient priority lives in the focus pointer

## Consequences

- **Resolves a long-standing anti-pattern.** Re-weighting ADRs to
  reflect current priorities pollutes durable intent with transient
  signal. The focus artefact carries the transient signal alone.
- **Closes the loop with GND-0006.** Backbriefing references current
  focus; work that touches out-of-focus domains is not forbidden but
  must be named as such, surfacing scope drift early.
- **Cost.** A new artefact to maintain. Mitigation: the artefact is
  short, time-boxed, and disposable by design.
- **Implementation latitude.** GND mandates the mechanism, not its
  form. A team may choose bd `phase:`/`roadmap` labels, milestone labels, OKR
  pointers, or equivalent; the obligation is that *some* artefact
  exists and meets R1–R4.
- **Observation mechanism (per GND-0005).** Lint enforcement: the
  focus artefact's presence, schema conformance, period currency,
  and resolution of in-focus directive IDs are checked by
  `adr-fmt`'s F001–F007 lints; absence or expiry surfaces as a
  diagnostic at every lint run.
