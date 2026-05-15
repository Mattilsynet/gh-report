# CHE-0046. Retry, Timeout, and Cancellation Semantics

Date: 2026-04-29
Last-reviewed: 2026-04-29
Tier: B
Status: Accepted

## Related

References: CHE-0021, CHE-0024, CHE-0041, COM-0025, GND-0001, GND-0004

## Context

Distributed command dispatch sees transient infrastructure failures, caller cancellation, retry storms, and ambiguous partial effects. Cherry-pit now exposes `ErrorCategory`, idempotency guidance, and checkpointed delivery, but no ADR states the retry contract. Three options were evaluated: automatic framework retries everywhere, caller-owned retry only, or bounded retries at infrastructure edges with idempotent commands. Option 3 preserves correctness while avoiding hidden duplicate side effects.

## Decision

Retry is explicit, bounded, and tied to idempotency. Infrastructure may retry retryable failures, but domain handlers remain deterministic and side effects are only committed through persisted events.

R1 [5]: CommandGateway retries only ErrorCategory::Retryable failures using bounded attempts, exponential backoff, jitter, and a total deadline
R2 [5]: CommandGateway treats ErrorCategory::Terminal failures as non-retryable and returns them without automatic redispatch
R3 [5]: Retried commands crossing HTTP, queue, scheduler, or webhook boundaries carry a stable idempotency key in the command payload
R4 [5]: EventBus publication retries use persisted EventEnvelope event_id values as deduplication keys for downstream consumers
R5 [5]: Cancellation of CommandGateway futures does not imply rollback after EventStore append succeeds; callers recover by reloading aggregate state
R6 [6]: Retry telemetry records attempt number, backoff duration, deadline, error category, aggregate_id, and correlation_id

## Consequences

Retry behavior becomes predictable and observable. The framework avoids exactly-once claims: duplicate suppression comes from idempotency keys, event IDs, and aggregate state. Callers must treat cancellation as unknown outcome once persistence may have happened.
