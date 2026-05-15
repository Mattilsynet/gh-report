# FLO-0013. Variability Pooling, Substitution, and Displacement

Date: 2026-05-03
Last-reviewed: 2026-05-03
Tier: B
Status: Accepted
Parent-cross-domain: PAR-0017 — variability-pooling-substitution-and-displacement is the FLO-tier expression of PAR-0017's universal directive that the state-machine bus is the canonical information path, by naming three concrete design rules for *where* variance is absorbed across pooled streams, substituted between cheap and expensive forms, and displaced toward the cheapest absorption stage

## Related

References: PAR-0017

## Context

Reinertsen V5: pooling uncorrelated streams reduces aggregate variance below
the sum of per-stream variances. V14: substitute cheap variability for
expensive — e.g. accept message-latency variance to reduce throughput variance.
V16: displace variance toward the stage with the cheapest absorption —
typically the stage with the largest buffer or the lowest cost of delay.
PAR-0017's state-machine bus is the runtime substrate where pooling,
substitution, and displacement decisions are realised: subject topology,
consumer-group structure, and buffer placement all express variance-placement
choices. No existing ADR names variance-placement as a design lever; this ADR
makes it explicit.

## Decision

Three design rules govern where variance is placed across stream topologies
built on PAR-0017's state-machine bus.

R1 [6]: When stream variances are uncorrelated, prefer pooling at a single
  consumer over per-stream consumers; correlated-variance streams remain
  separate so that pooling does not amplify a shared shock across the pooled
  aggregate.

R2 [6]: When two stages exhibit variance, displace it to the stage with the
  cheapest absorption — typically the stage with the largest buffer, the
  lowest cost-of-delay, or both — so the variance lands where its economic
  cost is smallest.

R3 [5]: Variance-placement decisions are documented per stream pair rather
  than implied by topology; the pair's pooling, substitution, and displacement
  choices are stated in the relevant CHE or PAR ADR rather than inferred from
  connection wiring.

## Consequences

Becomes easier: consumer-pooling decisions have a structural rationale;
per-pair variance budgets become writable; subject-design choices that affect
pooling are surfaced explicitly rather than embedded silently in topology.

Becomes harder: stream-pair documentation grows a variance-placement field;
topology-implicit assumptions must be made explicit when a pair is first
documented or amended.

Risks and migration: pooling correlated streams amplifies shocks — R1's
correlation guard is the primary mitigation. Migration: existing topologies
are documented opportunistically as they are touched; no retroactive sweep
is required.
