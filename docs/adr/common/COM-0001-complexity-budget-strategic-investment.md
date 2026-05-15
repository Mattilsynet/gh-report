# COM-0001. Complexity Budget — Strategic Investment

Date: 2026-04-25
Last-reviewed: 2026-04-28
Tier: S
Status: Accepted

## Related

Root: COM-0001

## Context

Ousterhout (Ch. 3) distinguishes tactical programming — shortcuts that accumulate into unmaintainable systems — from strategic programming, where every change invests 10–20% additional time in design quality that compounds over the system's lifetime. Cherry-pit adopted strategic programming from inception: 140+ ADRs written before most code, compile-fail tests verifying type contracts. This ADR formalizes the investment principle so all other decisions can cite it.

## Decision

Every design decision must justify its complexity cost against a
fixed budget. Zero tolerance for incremental complexity — no change
is too small to evaluate.

R1 [2]: Invest 10–20% additional time per task in design quality;
  this is the primary output, not optional overhead
R2 [2]: Before adding any abstraction, type parameter, or error
  variant, demonstrate the complexity is unavoidable — "it might
  be useful later" is not justification
R3 [2]: Each module and API surface has a finite complexity budget;
  additions that exceed it require refactoring to make room
R4 [3]: Red flags — "I'll clean it up later," interface mirrors
  implementation, caller passes computable information, error
  variant for a recoverable condition — trigger mandatory review

Complexity is assessed qualitatively through code review, not
quantitatively through metrics. The relevant question is: "Does a
developer reading this code for the first time need to understand
more concepts than the problem requires?"

## Consequences

The ADR system is a direct consequence — documenting "why" reduces cognitive load for future contributors. Compile-fail tests (CHE-0028, CHE-0038) are justified complexity investments eliminating entire classes of runtime errors. Every subsequent COM ADR applies the complexity budget: deep modules (COM-0002), pulling complexity down (COM-0003), error elimination (COM-0005). In distributed systems, interaction complexity (message ordering, partial failure, network partitions) is a multiplier on module-level complexity — the budget must account for both, which is why single-writer partitioning (COM-0018) eliminates an entire dimension of interaction cost. Tactical programming is not forbidden in emergencies, but tactical debt must be tracked and repaid.
