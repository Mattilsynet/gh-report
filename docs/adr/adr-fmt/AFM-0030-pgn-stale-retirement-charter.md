# AFM-0030. PGN Stale-Retirement Charter

Date: 2026-06-12
Last-reviewed: 2026-06-12
Tier: B
Status: Accepted

## Related

References: AFM-0022, AFM-0009, AFM-0029, AFM-0008

## Context

The PGN domain note recorded that GEN/PAR retirement and stale moves were
deferred while rescue material was consolidated. PGN-0001 through PGN-0014 are
Accepted and absorbed rescue ADR-0002 through ADR-0023 plus non-conflicting
legacy Solon GEN/PAR material. No prior ADR governed when that deferral could
end. Phase evidence records 11 EDGE and 15 DEMOTE classifications, 22 rows with
no clean successor, 18 prior UNCERTAIN rows demoted to narrative, and 7 already
terminal rows left untouched.

## Decision

R1 [5]: This ADR lifts the `adr-fmt.toml` GEN/PAR stale-retirement
  deferral for the pardosa-domain reconciliation.

R2 [5]: The trigger is completion of the PGN consolidation: PGN-0001
  through PGN-0014 are Accepted and carry the non-conflicting rescue
  and legacy Solon material.

R3 [5]: A stale ADR gets `Status: Superseded by PGN-NNNN` only when
  the whole ADR was carried into exactly one PGN.

R4 [5]: Partial supersession, two-PGN splits, many-to-one reorganizations,
  and dropped-subject ADRs receive narrative-only `## Retirement` prose
  naming what carried and what dropped.

R5 [5]: Lineage for this retirement lives only on the retired ADR's
  `Status:` field; active PGN ADRs gain no reciprocal `Supersedes:` edge.

R6 [5]: Rows without a clean successor, undecidable rows, and non-atomic
  rows use narrative retirement and never fabricate a PGN edge.

## Consequences

+ becomes easier: Phase 3 can retire GEN/PAR stale records under one explicit
  charter instead of reversing a config deferral silently.

− becomes harder: authors must preserve the atomicity evidence; a tempting
  partial or reciprocal edge is now an explicit policy violation.

risks/migration: apply AFM-0022 stub form during stale edits; lint remains the
  guardrail for malformed stubs and relationship vocabulary drift.
