# PAR-0009. LockedRescuePolicy Enum Replacing Bool

Date: 2026-04-25
Last-reviewed: 2026-04-28
Tier: D
Status: Accepted

## Related

References: PAR-0001

## Context

The original design gated `Locked → Rescue` behind an `acknowledge_data_loss: bool`. A boolean conveys intent but not semantics. With the new-stream migration model (PAR-0005), rescuing a locked fiber has nuanced audit trail semantics: old events may exist in the deprecated stream's grace period or may have expired.

## Decision

Replace `acknowledge_data_loss: bool` with:

```rust
pub enum LockedRescuePolicy {
    /// Old events remain in the deprecated stream's grace period.
    /// The audit trail is the deprecated stream itself.
    PreserveAuditTrail,
    /// Old events will be deleted when the deprecated stream expires.
    /// Caller acknowledges permanent data loss after the grace period.
    AcceptDataLoss,
}
```

The `rescue()` method takes `LockedRescuePolicy` instead of `bool`.
For `Detached → Rescue`, the policy is ignored — events remain in the
current stream and the precursor chain continues.

R1 [9]: Replace the acknowledge_data_loss bool parameter with a
  LockedRescuePolicy enum on the rescue method
R2 [9]: Derive Serialize and Deserialize on LockedRescuePolicy for
  persistence and configuration support

## Consequences

- API communicates audit trail semantics, not just acknowledgment.
- Enum is extensible — future variants added without changing the method signature.
- `LockedRescuePolicy` derives `Serialize`/`Deserialize` for persistence and configuration.
- Slightly more verbose call sites than `bool`.
- Removed `AcknowledgmentRequired` error — policy choice enforced at the type level.
