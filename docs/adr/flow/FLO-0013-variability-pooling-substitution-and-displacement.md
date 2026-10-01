# FLO-0013. Variability Pooling, Substitution, and Displacement

Date: 2026-05-03
Last-reviewed: 2026-10-01
Tier: B
Status: Accepted

## Related

References: FLO-0001

## Context

Reinertsen V5: pooling uncorrelated streams reduces aggregate variance below
the sum of per-stream variances. V14: substitute cheap variability for
expensive — e.g. accept message-latency variance to reduce throughput variance.
V16: displace variance toward the stage with the cheapest absorption —
typically the stage with the largest buffer or the lowest cost of delay.

The event-stream substrate is where pooling, substitution, and displacement
decisions are realised: subject topology, consumer-group structure, and buffer
placement all express variance-placement choices. Aligning with FLO-0001's
cost-of-delay discipline, variance-placement serves as an explicit design
lever for managing latency and throughput across stream pairs.

## Decision

Three design rules govern where variance is placed across stream topologies.

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
  choices are stated in the relevant stream design ADR rather than inferred from
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
