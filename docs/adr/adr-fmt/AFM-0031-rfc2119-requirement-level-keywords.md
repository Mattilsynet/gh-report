# AFM-0031. RFC-2119/8174 Requirement-Level Keywords as Normative Vocabulary

Date: 2026-07-16
Last-reviewed: 2026-07-16
Tier: S
Status: Accepted

## Related

References: AFM-0012, GND-0002, GND-0009

## Context

Tagged rules in `## Decision` sections use ad hoc phrasing for requirement
strength — "must", "should", "may" appear inconsistently, sometimes
lowercase, sometimes without any consistent meaning across ADRs. There is
no shared vocabulary distinguishing an absolute prohibition from a
recommendation. *RFC 2119 (Key words for use in RFCs to Indicate
Requirement Levels)* and *RFC 8174 (Ambiguity of Uppercase vs Lowercase
in RFC 2119 Key Words)* solve exactly this for specification documents;
adopting them gives the corpus a normative vocabulary agents can parse
unambiguously, orthogonal to the existing Meadows-layer classification
(AFM-0012).

## Decision

The corpus adopts the RFC 2119 / RFC 8174 keyword set as its normative
requirement-level vocabulary, uppercase-only, orthogonal to Meadows layer.

R1 [6]: Use MUST, MUST NOT, REQUIRED, SHALL, SHALL NOT, SHOULD, SHOULD
  NOT, RECOMMENDED, MAY, and OPTIONAL as the corpus's normative
  requirement-level vocabulary wherever a tagged rule states a
  requirement or prohibition
R2 [6]: Treat only the uppercase forms of these keywords as carrying
  normative force per RFC 8174; treat lowercase occurrences of the same
  words as ordinary prose with no normative meaning
R3 [6]: Hold the keyword axis orthogonal to the Meadows-layer axis —
  layer classifies intervention type, keyword classifies requirement
  strength; a high-layer rule does not imply MUST, and a MUST rule
  implies no particular layer
R4 [5]: Reserve keywords for tagged rules expressing a genuine
  requirement or prohibition per RFC 2119 §6 restraint, tying keyword use
  to the COM-0001 complexity budget rather than decorative emphasis
R5 [6]: Apply the keyword vocabulary to new and amended ADRs going
  forward; leave existing ADRs grandfathered and reword them
  opportunistically in place per AFM-0029:R2, with no flag-day migration

### Future work

Not implemented by this ADR: a candidate `adr-fmt --lint` diagnostic
(tentatively `T###`) checking keyword presence/usage in tagged rules, and
an opportunistic rewording of `TEMPLATE.md`'s tagged-rule guidance from
"positive imperative, unconditional" to "positive imperative, declared
requirement level". Both are follow-on scope, not part of this decision.

Observation mechanism per GND-0005: review-gate — reviewers check keyword
usage during normal ADR review, no automated enforcement yet.

## Consequences

+ becomes easier: agents and reviewers can parse requirement strength
  unambiguously from tagged-rule text, independent of Meadows layer.
+ becomes easier: future tooling (the candidate lint diagnostic) has a
  well-defined, RFC-anchored vocabulary to check against.
− becomes harder: rule authors must learn and consistently apply the
  RFC 2119/8174 vocabulary instead of free-form phrasing.
− becomes harder: mixed old/new phrasing persists during the
  grandfather period, so keyword presence cannot yet be treated as
  universal across the corpus.
risks/migration: no flag-day migration — existing ADRs are grandfathered
  and reworded opportunistically in place (AFM-0029:R2) when otherwise
  touched. No lint enforces keyword usage in this ADR; a future
  diagnostic is future work, not a commitment made here.
