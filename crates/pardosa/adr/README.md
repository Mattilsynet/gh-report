# pardosa — Architecture Decision Records

This directory records the architectural decisions in pardosa,
the EDA storage layer implementing fiber semantics. Each entry follows the
[Michael Nygard ADR format](https://cognitect.com/blog/2011/11/15/documenting-architecture-decisions).

## Scoping Rule

ADR numbering is **per-crate**. Pardosa ADRs start at 001 independently of
pardosa-genome's ADR numbering. To reference a genome ADR from a pardosa ADR,
use a relative path:

```
See [genome ADR-002](../../pardosa-genome/adr/0002-no-schema-evolution-fixed-layout.md)
```

## Index

| ADR | Decision | Status | Implementation |
|-----|----------|--------|---------------|
| [001](0001-fiber-state-machine-as-inspectable-data-table.md) | Fiber state machine as inspectable data table | Accepted | Phase 1 ✅ |
| [002](0002-index-none-sentinel-replacing-option.md) | Index::NONE sentinel replacing Option\<Index\> | Accepted | Phase 1 ✅ |
| [003](0003-event-immutability-private-fields-non-exhaustive.md) | Event immutability via private fields + #[non_exhaustive] | Accepted | Phase 1 ✅ |
| [004](0004-single-writer-per-stream.md) | Single-writer per stream | Accepted (amended) | Phase 1 ✅ (documented), Phase 5 (mandatory fencing) pending |
| [005](0005-new-stream-migration-model.md) | New-stream migration model | Accepted | Phase 4 pending |
| [006](0006-genome-as-primary-serialization.md) | Genome as primary serialization | Accepted (amended) | Phase 5 pending |
| [007](0007-monotonic-event-id-for-idempotent-publish.md) | Monotonic event_id for idempotent publish | Accepted | Phase 1 ✅ |
| [008](0008-publish-then-apply-durable-first.md) | Publish-then-apply with durable-first semantics | Accepted (amended) | Phase 5 pending |
| [009](0009-locked-rescue-policy-enum-replacing-bool.md) | LockedRescuePolicy enum replacing bool | Accepted | Phase 1 ✅ |
| [010](0010-fallible-constructors-replacing-debug-assert.md) | Fallible constructors replacing debug_assert | Accepted | Phase 1 ✅ |
| [011](0011-64-bit-target-requirement.md) | 64-bit target requirement | Accepted | Phase 1 ✅ |
| [012](0012-precursor-chain-verification-on-startup.md) | Precursor chain verification on startup | Accepted | Phase 2 ✅ |
| [013](0013-nats-kv-registry-for-atomic-stream-discovery.md) | NATS KV registry for atomic stream discovery | Accepted (amended) | Phase 5 pending |
| [014](0014-backpressure-and-circuit-breaker.md) | Backpressure and circuit breaker | Accepted | Phase 5 pending |

## Cross-Crate References

Pardosa and pardosa-genome share several cross-cutting invariants:

- **Frozen field order**: Types annotated with `GENOME LAYOUT` doc comments
  (`Event<T>`, `Fiber`, `Index`, `DomainId`) have declaration-order-sensitive
  genome binary layouts. Field reordering = schema break. See pardosa ADR-003
  and [genome ADR-001](../../pardosa-genome/adr/0001-serde-native-serialization-with-genomesafe-marker-trait.md).

- **Sentinel reservation**: `u64::MAX` is reserved by pardosa as `Index::NONE`.
  Genome's wire format must not assign structural meaning to this value.
  See pardosa ADR-002 and [genome ADR-002](../../pardosa-genome/adr/0002-no-schema-evolution-fixed-layout.md).

- **Feature-flag boundary**: Genome-dependent decisions are gated behind the
  `genome` feature in pardosa's `Cargo.toml`. See pardosa ADR-006.

---

*Provenance: ADRs 001–013 extracted from `pardosa.md`, `pardosa-next.md`,
`automerge-ideas.md`, and the implemented source code. April 2026.
ADR-014 added April 2026.*
