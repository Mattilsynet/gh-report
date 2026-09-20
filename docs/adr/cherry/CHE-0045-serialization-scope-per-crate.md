# CHE-0045. Serialization Scope Per Crate

Date: 2026-04-25
Last-reviewed: 2026-07-23
Tier: B
Status: Accepted

## Related

References: CHE-0029, CHE-0100

## Context

Cherry-pit is a multi-crate workspace (CHE-0029) spanning domain traits
(`cherry-pit-core`), infrastructure adapters (`cherry-pit-gateway`), and an optional
event serialization and storage layer (`pardosa`, `pardosa-genome`).
Two serialization decisions exist:

1. **CHE-0031** (superseded; retired lineage now covers pardosa-genome
   store encoding, not gateway format) — originally named MessagePack
   encoding for `cherry-pit-gateway`'s `MsgpackFileStore`. CHE-0100
   retired that store: neither it nor its `rmp-serde` dependency
   exists, so the gateway now owns no serialization format.
2. **PAR-0006** — pardosa-genome as the primary
   serialization for the `pardosa` crate. Optimised for zero-copy
   reads, compile-time schema hashing, and integrated compression.

Without explicit scoping, these decisions appear contradictory:
both claim to be the "primary" serialization format. In practice
they serve different crates with different performance and
compatibility requirements.

## Decision

Each crate owns its serialization strategy. No crate's choice
constrains another's.

R1 [5]: Each crate owns its serialization strategy independently; no
  crate may mandate a format for another crate
R2 [5]: Domain events are format-agnostic; the choice of serialization
  format is made at the infrastructure layer
R3 [5]: Feature flags gate serialization dependencies so users opt in
  explicitly

| Crate | Serialization | Governing ADR |
|-------|--------------|---------------|
| `cherry-pit-core` | None — domain traits are format-agnostic. `DomainEvent` carries no serde supertrait; `EventEnvelope` declares `#[serde(bound(...))]` so any serde backend works for events that opt in. | CHE-0010 |
| `cherry-pit-gateway` | None; store retired (CHE-0100). `MsgpackFileStore` and its `rmp-serde` dependency no longer exist. | CHE-0100 |
| `pardosa` | pardosa-genome as primary. MsgPack and JSON as feature-gated fallbacks for debugging and interop. | PAR-0006 |
| `pardosa-genome` | Defines the genome binary wire format. Serde-native with `GenomeSafe` marker trait. | GEN-0001 through GEN-0033 |
| `cherry-pit-web` | JSON via `serde_json` for HTTP API responses; the router declares its own `Serialize` bound on the aggregate's event type. Format determined by web conventions, not event storage. | CHE-0010 |

### Boundary Rules

1. **Domain events are format-agnostic.** A domain event type defined
   in user code works with any serde-compatible backend. The choice of
   MsgPack vs. genome vs. JSON is made at the infrastructure layer,
   not the domain layer.
2. **No crate may mandate a serialization format for another crate.**
   `cherry-pit-gateway` does not require genome. `pardosa` does not require
   MsgPack. Both coexist as alternative `EventStore` implementations.
3. **Feature flags gate serialization dependencies.** The `genome`
   feature in `pardosa` gates the `pardosa-genome` dependency. The
   `json` feature gates `serde_json`. Users opt in explicitly.
4. **Event envelope wire format is store-specific.** The `EventEnvelope`
   schema (CHE-0016, CHE-0042) is serialised by the store implementation,
   not by the domain. Different stores may use different encodings for
   the same logical envelope.

## Consequences

- No conflict between this ADR's `cherry-pit-gateway` row and PAR-0006 — they govern different crates. `cherry-pit-gateway` no longer has a serialization strategy of its own (store retired, CHE-0100); users choosing `pardosa` get genome.
- Domain event portability — `cherry-pit-core` is format-agnostic, so migrating from `MsgpackFileStore` to a future `GenomeFileStore` requires no domain code changes.
- Schema evolution strategies differ by crate: additive field evolution with `#[serde(default)]` (CHE-0022) for gateway, new-stream migration (PAR-0005) for pardosa.
- Two serialization strategies means two sets of golden-file tests and encoding-specific bug surfaces.
