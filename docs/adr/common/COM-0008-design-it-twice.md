# COM-0008. Design It Twice

Date: 2026-04-26
Last-reviewed: 2026-04-28
Tier: B
Status: Accepted
Parent-cross-domain: GND-0006 — designing-it-twice is the COM-tier expression of GND-0006's universal directive that intent be tested through backbriefing before action

## Related

References: GND-0006, COM-0001

## Context

Ousterhout (Ch. 11) observes that the first design is unlikely to be the best. Sketching at least two fundamentally different approaches exposes trade-offs a single-pass design misses. The cost of exploring a second design is minutes; discovering the first was wrong costs days. This applies to module interfaces, data representations, and API boundaries — not every function body.

Cherry-pit's ADR system institutionalizes this: CHE-0011 evaluated `u64`, `Uuid`, and `NonZeroU64`; CHE-0031 compared JSON, bincode, CBOR, and MessagePack; GEN-0007 evaluated Cap'n Proto, FlatBuffers, and a custom design.

## Decision

For any design decision at tier B or above, consider at least two
fundamentally different approaches before committing. Document the
alternatives and the rationale for selection.

R1 [5]: Before committing to a module interface or architectural
  boundary, sketch at least two approaches that differ in structure,
  not just in detail
R2 [6]: Evaluate each design against quality attributes — interface
  simplicity, information hiding, consistency, performance, safety,
  evolvability — to reveal trade-offs
R3 [5]: Time-box the exploration proportionally to tier; fifteen
  minutes for tier-B, a design document for tier-S
R4 [5]: The ADR Context section captures alternatives considered;
  the Decision section captures selection rationale

## Consequences

The ADR Context section structurally enforces this — an ADR presenting only one approach is incomplete. Code review for new traits and infrastructure ports can cite COM-0008 to request alternatives. The 10–20% strategic investment (COM-0001) explicitly covers multi-design exploration. Deliberate deferrals (CHE-0037, CHE-0040) are a specific application: "implement now" was the second design, evaluated and rejected. Risk of analysis paralysis is mitigated by rule 3's time-box.
