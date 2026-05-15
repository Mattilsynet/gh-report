# FLO-0006. Late Binding of Work to Writers

Date: 2026-05-03
Last-reviewed: 2026-05-03
Tier: A
Status: Accepted
Parent-cross-domain: PAR-0004 — late-binding-of-work-to-writers is the FLO-tier expression of PAR-0004's universal directive that each stream has a single writer, by deferring the choice of *which* stream a unit of work targets to dispatch time rather than submission time, so writer load becomes self-organising under variance

## Related

References: PAR-0004

## Context

Reinertsen F26 — bind demand to resources as late as economically possible. Early binding (route at submission) freezes the assignment before load is known, producing head-of-line blocking when the bound writer is busy while sibling writers idle. Late binding (route at dispatch) lets the runtime pick the most economic writer at the moment of work, per F22/F23/F25. PAR-0004 fixes writer-per-stream as a structural invariant — the *writer* is fixed per stream — but the *stream* a unit of work targets may be late-bindable when the work expresses a logical destination rather than a stream key. This is a self-organisation primitive (Tier A) because it changes what is dynamically routable in the runtime.

## Decision

Work submission MAY express a logical destination resolved at dispatch time; early binding to a stream identifier at submission time is the exception rather than the default.

R1 [4]: Work submission MAY express a logical destination — for example an aggregate kind plus key — that the runtime resolves to a stream at dispatch time, rather than requiring the submitter to commit to a stream identifier at submission time.
R2 [5]: Early binding to a specific stream remains permitted but is justified in Context — for example when an idempotency key requires a stable destination — so late binding is the default and early binding is the exception.
R3 [6]: Late-binding resolution is a single named runtime concern with its own observability surface (decisions per second, dispatch-time CoD, resolved-stream distribution) so operators can audit routing fairness and detect resolver imbalance.

## Consequences

+ becomes easier: load-aware dispatch, alternate-route-around-congestion patterns, and CoD-weighted routing all become expressible. Head-of-line blocking against a single busy writer no longer requires submission-side awareness.
- becomes harder: the dispatcher acquires a routing concern; logical destinations require a resolver per kind. Idempotency stories must consider resolver determinism.
risks/migration: a non-deterministic resolver under retry causes split-brain — R2's early-binding exception covers idempotency-critical work; R3's observability surfaces resolver drift. Migration: existing eagerly-bound submitters keep working unchanged; new submission paths default to logical destinations.
