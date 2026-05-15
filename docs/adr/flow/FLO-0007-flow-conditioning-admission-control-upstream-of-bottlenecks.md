# FLO-0007. Flow Conditioning and Admission Control Upstream of Bottlenecks

Date: 2026-05-03
Last-reviewed: 2026-05-03
Tier: B
Status: Accepted
Parent-cross-domain: PAR-0014 — flow-conditioning-upstream-of-bottlenecks is the FLO-tier expression of PAR-0014's universal directive that backpressure protects writers, by adding a proactive admission-shaping layer upstream of the writer that smooths arrival variance before it reaches the constraint rather than reacting after saturation

## Related

References: PAR-0014

## Context

Reinertsen F30 — reduce variability before the bottleneck; FF18 — feedforward gives advance notice of heavy arrival rates so the bottleneck can prepare; Q7/Q8 — variance arriving at a single server is amplified, especially when adjacent queues are linked. PAR-0014 is reactive backpressure: it refuses arrivals after the queue saturates. Flow conditioning is proactive admission shaping: it delays, batches, or reorders arrivals upstream so variance does not reach the constraint at peak amplitude. In a single-writer-per-stream architecture, the admission point sits between the publisher and the writer; conditioning is its responsibility, not the writer's.

## Decision

Each single-writer stream exposes a named admission point upstream of the writer; the admission point applies a configurable shaping policy before arrivals reach the writer, and may consume feedforward signals from downstream consumers.

R1 [6]: Each single-writer stream exposes an admission point upstream of the writer where arrivals MAY be delayed, batched, or reordered before reaching the writer; the admission point is a named runtime concern with its own configuration surface.
R2 [8]: Downstream consumers MAY emit feedforward signals — for example expected arrival rate over the next cadence period — that are consumed by the admission point so conditioning anticipates load rather than only reacting to it.
R3 [6]: Admission shaping does not break event ordering within a stream; reorder operations are permitted only across logical destinations that the late-binding resolver (the FLO-0006 surface) treats as equivalent.

## Consequences

+ becomes easier: variance smoothing without writer-side awareness; feedforward-aware capacity planning; per-stream conditioning policies.
- becomes harder: the publish path acquires a conditioning concern; ordering invariants must be re-stated when admission may reorder.
risks/migration: a reordering admission policy that violates the writer's ordering contract is a correctness bug, not a performance issue — R3 is the explicit guard. Migration: existing publishers acquire a no-op admission point initially; conditioning policies graduate per-stream.
