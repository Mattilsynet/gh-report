# CHE-0079. No Bespoke Ops Console

Date: 2026-06-13
Last-reviewed: 2026-06-13
Tier: B
Status: Proposed
Parent-cross-domain: COM-0019 — observability-boundary authority is the Common-domain telemetry doctrine this CHE absence ratifies

## Related

References: COM-0019, CHE-0024, CHE-0051, CHE-0047, CHE-0046, CHE-0001

## Context

cherry-pit-app already emits back-pressure and dispatch failure signals via
structured tracing (`crates/cherry-pit-app/src/app.rs:342-390`). Its default
dead-letter sink records event, correlation, causation, error category, output,
and policy identity fields (`crates/cherry-pit-app/src/dead_letter.rs:130-170`).
COM-0019 governs the telemetry boundary. The remaining question is whether
Solon ships a hosted operational view.

## Decision

Ratify the absence of a bespoke ops console. P1 correctness and P2
confidentiality win over convenience: the substrate emits bounded signals, and
consumers own aggregation and display.

R1 [5]: Do not ship a hosted message-flow, processor-health, or dead-letter dashboard service from the substrate

R2 [6]: Surface operational health through structured traces, logs, metrics, and dead-letter records governed by COM-0019

R3 [6]: Keep event_id, aggregate_id, correlation_id, and causation_id in spans, logs, or records, not metric labels

R4 [5]: Let consumers choose exporters, retention, alert thresholds, and visualisations outside the substrate API

R5 [5]: Treat dispatch back-pressure as a traced drop or dead-letter signal, not a console-owned control loop

R6 [5]: Keep recovery procedures in runbooks and durable records rather than an always-on repair console

R7 [5]: Require any future operator surface to consume existing signals without introducing new high-cardinality metrics

## Consequences

+ becomes easier: the substrate keeps one observability contract, and consumers
  can integrate it with their existing exporter and runbook practices.

− becomes harder: no turnkey console exists for local demonstrations or ad hoc
  operator inspection; users must wire their own aggregation.

risks/migration: this Proposed ADR adds no Rust code. If a consumer later needs
  a packaged view, it must be justified as a separate surface consuming these
  signals rather than replacing them.
