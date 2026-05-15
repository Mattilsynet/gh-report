# COM-0022. Pre-validate Then Commit

Date: 2026-04-28
Last-reviewed: 2026-04-28
Tier: B
Status: Accepted

## Related

References: COM-0018, GND-0005

## Context

Database ACID transactions validate constraints before committing. Copy-on-write filesystems (ZFS, Btrfs) write new state to a fresh location and atomically commit. Greg Young's event sourcing formalizes this: command handling is a decision function (may fail), event logging is the commit, and apply is a deterministic fold (cannot fail). Cherry-pit applies pre-validate-then-commit everywhere: Dragline pre-computes values before mutation, MsgpackFileStore uses atomic temp-file-then-rename, EventEnvelope validates in the constructor. Single-writer architecture eliminates contention that makes optimistic approaches attractive.

## Decision

Perform all fallible computation before any state mutation. If
validation or preparation fails, no observable state has changed.

R1 [5]: Mutation methods compute all derived values and validate
  all preconditions before the first assignment to mutable state
R2 [5]: File and storage writes use atomic replacement (temp file
  then rename) so readers never observe partial writes
R3 [5]: Constructor methods return Result and perform all validation
  before yielding the constructed value to the caller
R4 [6]: When a sequence of related mutations must succeed or fail
  together, compute the full result set first and apply all changes
  in a non-failing final step

## Consequences

Crash safety improves — interruption at any point either leaves old state intact or completes the full transition. Debugging simplifies because the system is never in a half-mutated state. The cost is computational: values are computed before needed, and temporary storage (intermediate variables, temp files) is required. Rollback logic is unnecessary because failure leaves original state untouched. The principle does not apply to CAS-loop patterns where retry is the intentional recovery mechanism.
