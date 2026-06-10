# CHE-0072. pardosa Backend Config Knob

Date: 2026-06-10
Last-reviewed: 2026-06-10
Tier: B
Status: Accepted
Crates: gh-report
Parent-cross-domain: PGN-0010 — backend selection uses pardosa sealed handles

## Related

References: PGN-0010, CHE-0044, CHE-0071, CHE-0048, CHE-0054

## Context

gh-report needs an operator-visible way to choose the pardosa authoritative backend without turning storage into runtime-pluggable topology. Oracle Q5 ruled that a startup selector choosing a sealed pardosa handle while preserving one concrete `EventStore` type complies with STORY §7. The initial implementation must keep `.pgno` working and make incomplete NATS wiring explicit.

## Decision

gh-report carries a `PardosaBackend` enum in `RuntimeConfig`, surfaced through `--pardosa-backend pgno|nats` and `GH_REPORT_PARDOSA_BACKEND`. Startup resolves the enum into a sealed pardosa handle constructor feeding the concrete `PardosaEventStore<DomainEvent>` adapter; projection rebuild stays backend-agnostic through `load()` and `list_aggregates()`.

R1 [5]: `RuntimeConfig::pardosa_backend` is the sole gh-report storage-backend selector, defaulting to `Pgno` and set by the CLI flag or environment variable.
R2 [5]: The selector MUST resolve at startup to the concrete `PardosaEventStore<DomainEvent>` type; `Box<dyn EventStore>` and generic backend parameters in gh-report composition are forbidden.
R3 [5]: The `Pgno` backend writes `<store_dir>/events/<org>/events.pgno` while preserving the existing projection snapshot and checkpoint paths.
R4 [5]: The `Nats` backend MAY be parsed before runtime-handle wiring exists, but M1 MUST return an explicit startup error rather than silently falling back to `.pgno`.
R5 [5]: Projection bootstrap MUST remain backend-agnostic by calling the adapter's `list_aggregates()` and `load()` methods, not by reading `.pgno` files or JetStream cursor state directly.
R6 [5]: JetStream semantics are latest-message-wins full-blob rehydration through `open_with_backend`; gh-report MUST NOT use cursor or file-tail paths for projection rebuild.

## Consequences

+ becomes easier: operators get one visible storage selector and tests can assert `.pgno` artifacts without reworking the application topology.
− becomes harder: live NATS startup requires a follow-up to supply a runtime handle and a provisioned-NATS conformance run.
risks/migration: `MsgpackFileStore` remains in-tree per CHE-0044 coexistence, but gh-report production wiring moves to the pardosa adapter. The live-NATS follow-up is tracked in bd `adr-fmt-2v35g`.
