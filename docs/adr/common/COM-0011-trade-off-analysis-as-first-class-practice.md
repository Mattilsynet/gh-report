# COM-0011. Trade-Off Analysis as First-Class Practice

Date: 2026-04-26
Last-reviewed: 2026-04-28
Tier: B
Status: Accepted

## Related

References: COM-0001, GND-0001

## Context

Richards and Ford (Fundamentals of Software Architecture, Ch. 2): "Everything in software architecture is a trade-off." This is distinct from COM-0001 (how much complexity is justified) — trade-off analysis addresses which qualities compete and which side this project chooses.

Cherry-pit's trade-offs: correctness over performance (CHE-0028, CHE-0009, CHE-0032), simplicity over flexibility (CHE-0037, CHE-0040, GEN-0031), safety over ergonomics (CHE-0021, CHE-0007), consistency over local optimality (CHE-0015, CHE-0009, COM-0009). The ADR system documents trade-offs structurally; this ADR makes the practice mandatory.

## Decision

Every architectural decision is a trade-off. The competing quality
attributes and the rationale for the chosen balance must be
explicit in the decision record.

R1 [5]: Reject decision framing that claims an approach is
  universally superior; reframe as "optimizes for X at cost of Y"
R2 [5]: Every ADR Context section must identify at least two quality
  attributes in tension
R3 [6]: Cherry-pit's priority ordering — correctness then safety
  then simplicity then performance then flexibility — is the
  standing resolution for common trade-offs
R4 [5]: The Consequences section must state what was given up, not
  just what was gained
R5 [6]: When project scale or requirements shift, review affected
  ADRs against their trade-off assumptions

## Consequences

ADR Context and Consequences sections are the enforcement — an ADR presenting only benefits without costs is incomplete. CHE-0001 (design priority ordering) becomes the standing trade-off resolution for common quality attribute conflicts. Deliberate deferrals (CHE-0037, CHE-0040, GEN-0031) are validated as explicit trade-offs with documented revisit conditions. COM-0008 (Design It Twice) becomes more productive when comparison axes are explicit. In distributed systems, the CAP theorem and PACELC model provide additional standing trade-off axes that every persistence and communication decision must address. Risk of over-analysis is mitigated by COM-0008's time-box: proportional to tier and reversibility.
