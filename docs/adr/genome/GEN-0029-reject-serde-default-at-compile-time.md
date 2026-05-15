# GEN-0029. Reject #[serde(default)] at Compile Time

Date: 2026-04-25
Last-reviewed: 2026-04-25
Tier: B
Status: Accepted

## Related

References: GEN-0004

## Context

`#[serde(default)]` provides default values for absent fields in self-describing formats (JSON, TOML). In pardosa-genome's fixed-layout format, fields are always present at compile-time-determined offsets — `default` compiles but has no effect, creating a silent behavioral difference. GEN-0004 establishes the pattern of rejecting incompatible serde attributes. Silently accepting `default` misleads developers migrating from JSON who expect missing-field defaulting behavior that never activates.

## Decision

`#[derive(GenomeSafe)]` rejects `#[serde(default)]` at compile time with
a clear error message explaining that fixed-layout formats have no concept
of missing fields.

**Error message:**
```
GenomeSafe: #[serde(default = "...")] is not supported on field.
Fixed-layout format has no concept of missing fields. All fields are
always present at their compile-time offsets.
```

The `default` path attribute (type-level `#[serde(default)]`) is also
rejected with the same rationale.

R1 [5]: derive(GenomeSafe) rejects serde(default) at compile time with
  a clear error message
R2 [5]: Both field-level and type-level serde(default) are rejected
R3 [6]: Fixed-layout formats have no concept of missing fields — all
  fields are always present at their compile-time offsets

## Consequences

- **Positive:** Prevents silent behavioral divergence from JSON/TOML usage — clear compile-time error instead of a no-op. Aligns with GEN-0004 pattern.
- **Negative:** Users migrating existing serde types must remove `#[serde(default)]` before adding `GenomeSafe`. Intentional friction.
- **Negative:** Types used with both JSON and pardosa-genome require conditional compilation or separate type definitions.
- **Residual risk:** Both field-level and type-level `#[serde(default)]` are caught by the same attribute scanning logic.
