# CHE-0001. Design Priority Ordering

Date: 2026-04-25
Last-reviewed: 2026-04-25
Tier: S
Status: Accepted

## Related

Root: CHE-0001

## Context

Cherry-pit makes many tradeoff decisions — type strictness vs ergonomics, runtime checks vs performance, dependency count vs convenience. Without a shared ranking, each decision becomes a local argument with no resolution criterion. Existing ADRs already reference correctness-over-speed (CHE-0026), no unsafe (CHE-0007), and pure command handling (CHE-0008), pointing to an implicit priority system never formally established. Four candidate priorities compete: correctness, security, energy efficiency, and response time. The question is their strict rank order when they conflict.

## Decision

Every design decision is evaluated against these priorities in strict
rank order:

| Priority | Name | Principle |
|----------|------|-----------|
| P1 | Correctness | Make illegal states unrepresentable. Lean on the type system. Total functions. No unsafe. |
| P2 | Secure | No data leakage across bounded contexts. Validate at boundaries. |
| P3 | Energy efficient | Do less work, not faster work. Avoid unnecessary allocations, cloning, serialization. |
| P4 | Response time | Fast, but never at the cost of correctness. |

"Strict rank order" means: when P1 and P4 conflict, P1 wins without
debate. When P2 and P3 conflict, P2 wins. No decision may optimize a
lower-priority concern at the expense of a higher one.

This ADR is the canonical source for the priority system. Other ADRs
cite it by number when invoking a priority tradeoff.

R1 [1]: Evaluate every design decision against the priority ordering
  P1 Correctness > P2 Security > P3 Energy efficiency > P4 Response time
R2 [1]: When two priorities conflict the higher-ranked priority wins
  without debate
R3 [1]: Every ADR must cite the priority tradeoff that drove the
  decision

## Consequences

Concrete tradeoffs driven by this ordering: overflow-checks in release (CHE-0026, P1 > P4), forbid unsafe (CHE-0007, P1), pure command handling (CHE-0008, P1), associated types over trait objects (CHE-0005, P1), single-aggregate typing (CHE-0005, P2), RPITIT over async_trait (CHE-0025, P3), and `DeserializeOwned` on `DomainEvent` (CHE-0010, P1 > P3). New ADRs must cite which priority won and which was sacrificed. The ranking is intentionally opinionated — projects prioritizing response time over correctness should not adopt cherry-pit.
