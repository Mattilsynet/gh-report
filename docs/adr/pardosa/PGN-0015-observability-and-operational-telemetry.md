# PGN-0015. Observability and Operational Telemetry

Date: 2026-06-12
Last-reviewed: 2026-06-12
Tier: B
Status: Accepted
Crates: pardosa, pardosa-nats

## Related

References: PGN-0010, COM-0019

## Context

Pardosa now admits NATS/JetStream as authoritative storage, but no live ADR governs storage-path observability after PAR-0023 was retired. This closes the GND-0001 governing gap while preserving PGN-0010's byte-verbatim boundary. It re-rescues PAR-0023's bounded-label budget, including the <=500 active label-combinations-per-metric ceiling, while balancing diagnosability, confidentiality, runtime cost, and canonical-integrity safety.

## Decision

Operational telemetry is part of the pardosa storage contract, but it remains read-only with respect to canonical bytes, frontier state, and public façade shape. Metrics carry bounded labels; high-cardinality identifiers and retry details remain reconstructible through spans or logs.

R1 [6]: `append`, `sync`, `replay`, and `connect` storage operations emit spans at backend entry and completion, including success or typed terminal failure.
R2 [6]: The storage path exposes at least one counter and one histogram, covering publish-ack timeouts and append latency or equivalent operational signals.
R3 [6]: Metric labels use a bounded vocabulary; per COM-0019:R6, `event_id`, `AckPosition`, stream, and `correlation_id` remain spans/log fields, never unbounded label dimensions.
R4 [6]: Retry telemetry follows COM-0019:R7: each retried storage operation emits attempt count, terminal category, and `correlation_id` in structured spans or logs.
R5 [5]: Instrumentation may read subjects, headers, and `AckPosition` for telemetry only; per PGN-0010:R4, those metadata never influence frontier CRH or canonical encoding.
R6 [5]: Instrumentation lives inside the synchronous-facade backend boundary; per PGN-0010:R5, public pardosa storage APIs do not gain async functions or runtime handles.
R7 [5]: Telemetry dependencies land in the pardosa adapter ring; per PGN-0001:R2, `crates/pardosa-nats` remains free of runtime-ring dependency pressure.
R8 [5]: All instrumentation preserves PGN-0001:R3: no unsafe carve-outs, code generation bypasses, or telemetry libraries that require crate-level unsafe exceptions.

## Consequences

+ becomes easier: downstream storage instrumentation can add spans, counters, and histograms without re-deciding byte-verbatim storage or façade constraints.
− becomes harder: metric-label expansion, metric-only debugging of high-cardinality failures, and forcing telemetry dependencies into `crates/pardosa-nats`.
risks/migration: PAR-0023 remains retired without a reciprocal supersedes edge; its <=500 active label-combinations-per-metric ceiling is carried here as an operational budget. If metrics lose diagnostic granularity or threaten metric-store cost, revise the budget before widening labels.
