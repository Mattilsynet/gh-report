# PAR-0020. Lease via NATS KV CAS

Date: 2026-04-29
Last-reviewed: 2026-04-29
Tier: A
Status: Accepted

## Related

References: PAR-0004

## Context

PAR-0004 mandates a single writer per stream. Without an enforced
lease, a stale writer surviving a network partition or GC pause can
double-publish, breaking PAR-0007's monotonicity. JetStream itself
provides no native lease primitive, but its KV store offers
linearizable Create and CAS Update operations backed by Raft.

The ripienaar/nats-kv-leader-elect reference implementation (Go)
demonstrates that KV revision numbers serve as fencing tokens when the
leader periodically renews the key with `Update(key, value,
expected_revision)`. TTL on the bucket reclaims abandoned leases
automatically. Renewal interval at 0.75 × TTL gives three renewal
attempts before expiry — the standard lease-renewal heuristic.

## Decision

Pardosa writers hold their stream lease as a key in a TTL-bounded NATS
KV bucket. The KV revision returned by Create or Update is the fencing
token attached to every JetStream publish. A writer steps down on any
Update revision mismatch before abandoning in-flight reservations.

R1 [5]: Acquire the writer lease by calling kv.create on the lease key
  pardosa.registry.{name}.lease and treat success as election to leader
R2 [5]: Renew the lease by calling kv.update with the last observed
  revision at an interval of zero point seven five times the bucket TTL
R3 [5]: Treat the revision returned by kv.create or kv.update as the
  fencing token and attach it to every JetStream publish for the
  duration of the lease
R4 [8]: Stop admitting new reservations and abandon all in-flight
  reservations the moment a kv.update call returns a revision mismatch
R5 [12]: Set the PARDOSA_LEASE bucket TTL to sixty seconds and the
  initial campaign splay to a uniform random delay between zero and
  five seconds

## Consequences

Single-writer fencing becomes provable from KV semantics — the broker
rejects publishes that carry a stale token. Stepping down before
abandoning reservations preserves PAR-0008's apply-after-ack invariant
on the failure path. Trade-offs: lease renewal traffic adds a small
constant load on the registry cluster. A pause longer than TTL forfeits
the lease even if the underlying network is healthy; this is the
intended trade — bounded uncertainty over correctness.
