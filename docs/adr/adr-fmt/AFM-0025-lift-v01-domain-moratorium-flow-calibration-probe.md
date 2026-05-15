# AFM-0025. Lift v0.1 Domain Moratorium for `flow/` Calibration Probe

Date: 2026-05-03
Last-reviewed: 2026-05-03
Tier: B
Status: Accepted

## Related

References: AFM-0008, AFM-0001

## Context

The eight retained domains validate that `adr-fmt --lint` generalises
beyond its originating corpus. Reinertsen's *Principles of Product
Development Flow* (2009) is a coherent, standalone canon — a peer to
Ousterhout (COM) and Meadows (AFM-0011) — whose flow-control vocabulary
maps directly onto CHE/PAR scheduling concerns. The corpus has been
stable long enough to absorb a ninth domain without surface drift. The
moratorium's intent — no domain churn during v0.1 — is preserved by
naming this exception explicitly, bounding it to a single probe, and
requiring a dedicated ADR.

## Decision

Add a single named `flow/` domain as a bounded exception to the v0.1
domain moratorium. No further domain additions are authorised in v0.1;
this exception is narrow, time-boxed, and fully attributed.

R1 [5]: Add the `flow/` (FLO) domain to `adr-fmt.toml` as the sole named
  exception to the FOCUS-level v0.1 domain moratorium; no further domain
  additions are permitted before v0.1 ships.
R2 [5]: The `flow/` domain hosts ADRs derived exclusively from Reinertsen's
  *Principles of Product Development Flow* (2009) and is bounded to that
  source scope for the v0.1 period.
R3 [6]: Treat `flow/` as the v0.1 generalisation probe — lint-cleanliness
  across all nine domains is the evidence that the binary generalises
  beyond the originating corpus, as stated in FOCUS.md.
R4 [5]: When v0.2 begins, this exception lapses; AFM-0025 does not
  pre-authorise further v0.1 additions, and any new domain in v0.2
  requires its own domain-policy ADR under the v0.2 governance period.

## Consequences

+ becomes easier: CHE/PAR flow-control ADRs gain a structural parent
  surface (FLO-) grounded in a single coherent Reinertsen canon; the
  binary's generalisation claim is testable against a ninth independent
  domain.
− becomes harder: "exactly eight domains" is no longer accurate in
  README and onboarding materials; nine must be stated explicitly.
risks/migration: The exception is named and narrow — this ADR explicitly
  does not pre-authorise further moratorium-lifting; future exceptions
  require their own AFM ADRs. Concurrent v0.1 work must not treat this
  carve-out as a pattern permitting additional domains.
