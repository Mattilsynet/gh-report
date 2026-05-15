# AFM-0024. Agent Critique via `--refs`

Date: 2026-05-03
Last-reviewed: 2026-05-03
Tier: B
Status: Accepted

## Related

Supersedes: AFM-0021
References: AFM-0012, AFM-0011

## Context

AFM-0024 defines `--refs`, a single-purpose
reverse-reference query. The rationale was that agents can read ADR
bodies directly; the binary's role is to answer derived questions the
raw corpus cannot cheaply provide.

The asymmetric T019 rule (AFM-0012:R4, as reshaped by this ADR's
package) surfaces structural tension only when a rule operates at
*higher* leverage than its ADR tier warrants. This replaces the
symmetric `abs_diff` approach and removes the `process_governance`
carve-out. The new semantics make `--lint` the sole authority for
tier-tension diagnostics; `--refs` remains a pure graph query.

## Decision

Agents can perform ADR critique by composing `--refs` calls with
targeted file reads; the binary does not inline bodies or compute
tier-tension summaries inline.

R1 [5]: An agent critiquing ADR-X issues `adr-fmt --refs ADR-X` to
  obtain the reverse-reference list, then reads individual referrer
  files selectively based on tier rank, status, and relevance — the
  binary's role is the graph projection, not content aggregation
R2 [5]: The asymmetric T019 rule (AFM-0012:R4) is the authoritative
  tier-tension diagnostic; agents surface T019 findings via
  `adr-fmt --lint` rather than by computing tension inline in the
  critique loop
R3 [6]: `adr-fmt --refs ADR-X` returns a one-bullet-per-referrer
  markdown list (per AFM-0021:R1–R4) without inlining body content;
  agents that need body content follow up with direct file reads,
  preserving context-window budget

## Consequences

- **Composability preserved.** The agent critique loop (enumerate
  referrers → read selectively → surface tension via `--lint`) is
  deterministic and parallelisable; no binary-side aggregation.
- **T019 is the tier-tension authority.** Removing the
  `process_governance` carve-out from T019 means all domains are
  evaluated under the same asymmetric rule; agents need not track
  domain-specific tolerance overrides.
- **AFM-0021 retired.** This ADR
  documents the agent-side workflow contract layered atop that
  surface. AFM-0021 is superseded and relocated to `docs/adr/stale/`
  per AFM-0022:R1–R2.

## Tier classification footnote

Codifies an agent-side workflow rule over an unchanged CLI surface.
Tier B (Design) per AFM-0011 R1 first-yes-wins: AFM-0024 specifies
information flow between agent and binary (layer 5–6) but does not
change the binary's extensibility seam, which remains pinned by
AFM-0021 (A). Tier downgrade on supersession is correct — AFM-0021
*created* the seam; AFM-0024 *uses* it.
