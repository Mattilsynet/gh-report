# CHE-0072. pardosa Backend Config Knob

Date: 2026-06-10
Last-reviewed: 2026-06-11 — refined — fold-all-N per-event-frame rehydration replaces latest-message-wins full-blob; canonical bytes unchanged
Tier: B
Status: Accepted
Crates: gh-report
Parent-cross-domain: PGN-0010 — backend selection uses pardosa sealed handles

## Related

References: PGN-0010, CHE-0074, CHE-0048, CHE-0073

## Context

gh-report needs an operator-visible way to choose the pardosa authoritative backend without turning storage into runtime-pluggable topology. Oracle Q5 ruled that a startup selector choosing a sealed pardosa handle while preserving one concrete `EventStore` type stays within the compile-time-composition constraint (CHE-0005:R1) and the no-runtime-pluggable-architectures non-goal. The initial implementation must keep `.pgno` working and make incomplete NATS wiring explicit.

## Decision

gh-report carries a `PardosaBackend` enum in `RuntimeConfig`, surfaced through `--pardosa-backend pgno|nats` and `GH_REPORT_PARDOSA_BACKEND`. Startup resolves the enum into a sealed pardosa handle constructor feeding gh-report's native pardosa store port; projection rebuild stays backend-agnostic through `load()` and `list_aggregates()`.

R1 [5]: `RuntimeConfig::pardosa_backend` is the sole gh-report storage-backend selector, defaulting to `Pgno` and set by the CLI flag or environment variable.
R2 [5]: The selector MUST resolve at startup to the concrete native store port over `pardosa::store::EventStore<gh_report::event::DomainEvent>`; `Box<dyn EventStore>` and generic backend parameters in gh-report composition are forbidden.
R3 [5]: The `Pgno` backend writes `<store_dir>/events/<org>/events.pgno` while preserving the existing projection snapshot and checkpoint paths.
R4 [5]: The `Nats` backend MAY be parsed before runtime-handle wiring exists, but M1 MUST return an explicit startup error rather than silently falling back to `.pgno`.
R5 [5]: Projection bootstrap MUST remain backend-agnostic by calling the adapter's `list_aggregates()` and `load()` methods, not by reading `.pgno` files or JetStream cursor state directly.
R6 [5]: JetStream rehydration replays all per-event-frame messages in stream-sequence order and folds each message body, one event's canonical bytes, into the reconstructed line; the stream holds one message per event, not a single republished growing blob, modulo an optional single zero-event seed record. gh-report MUST NOT use cursor or file-tail paths for projection rebuild.
R7 [5]: For `Nats`, gh-report derives one JetStream stream per org per PGN-0010:R6: token = `org_` plus lowercase hex of the exact UTF-8 org name bytes; `stream_name` = `gh-report-{token}`, `subject` = `gh-report.{token}.events`, and `durable_consumer` = `gh-report-{token}`.
R8 [5]: The org-name token MUST be injective and NATS-safe per PGN-0010:R4: no case folding or lossy replacement; empty org names are rejected; distinct org names MUST NOT collide on `stream_name` or `subject`, and spaces, dots, `*`, `>`, and every other non-token byte are encoded, not copied.
R9 [5]: gh-report surfaces the JetStream per-operation timeout as a transition-stopgap config/env knob, defaulting to current behaviour, for booting legacy large streams during migration to per-event frames; the knob is not the fix for quadratic growth, which R6 resolves structurally.

## Consequences

+ becomes easier: operators get one visible storage selector and tests can assert `.pgno` artifacts without reworking the application topology.
− becomes harder: live NATS startup requires a follow-up to supply a runtime handle and a provisioned-NATS conformance run.
risks/migration: `MsgpackFileStore` remains in-tree per CHE-0044 coexistence, but gh-report production wiring moves to the native pardosa store port. The live-NATS follow-up is tracked in bd `adr-fmt-2v35g`.
