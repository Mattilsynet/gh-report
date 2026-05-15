# ADR-009: LockedRescuePolicy enum replacing bool

**Status:** Accepted

### Context

The original design gated `Locked → Rescue` behind an
`acknowledge_data_loss: bool` parameter. A boolean conveys intent ("I know
this is destructive") but not semantics ("what happens to the old data?").

With the new-stream migration model (ADR-005), rescuing a locked fiber has
nuanced audit trail semantics: the old events may still exist in the
deprecated stream's grace period, or they may have already expired.

### Decision

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

### Consequences

- **Positive:** API communicates audit trail semantics, not just
  acknowledgment.
- **Positive:** Enum is extensible — future variants (e.g.,
  `ArchiveToAuditStream`) can be added without changing the method signature.
- **Positive:** `LockedRescuePolicy` derives `Serialize`/`Deserialize` for
  persistence and configuration.
- **Negative:** Slightly more verbose call sites than `bool`.
- **Negative:** Removed `AcknowledgmentRequired` error variant — the API
  now enforces policy choice at the type level rather than returning an error.

### References

- `src/fiber_state.rs`: `LockedRescuePolicy`
- `src/dragline.rs`: `rescue()` method
- pardosa-next.md §P4
