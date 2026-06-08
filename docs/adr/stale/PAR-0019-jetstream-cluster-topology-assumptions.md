# PAR-0019. JetStream Cluster Topology Assumptions

Date: 2026-04-29
Last-reviewed: 2026-04-29
Tier: B
Status: Accepted

## Related

References: PAR-0004

## Context

Pardosa relies on JetStream for both stream durability and the KV
registry (PAR-0013). Several correctness properties — single-writer
fencing (PAR-0004), idempotent publish (PAR-0007), per-subject expected
sequence headers — depend on cluster topology and server version.
nats-server issue #7361 (Sep 2025) demonstrated that
`Nats-Expected-Last-Subject-Sequence` was previously compared against
the stream-global last sequence, breaking per-fiber concurrency control.
Without recorded assumptions, deployments may run unsupported topologies
that silently violate invariants.

## Decision

Pardosa documents and enforces minimum cluster requirements. Streams use
replication factor 3 across distinct availability zones. The KV registry
runs on the same cluster as the stream it points to. The supported
nats-server version is fixed at the lowest release containing the
fix for issue #7361 and is encoded as a startup precondition check.

R1 [6]: Configure pardosa-managed JetStream streams with replicas equal
  to three across distinct availability zones via the StreamConfig
  passed to jetstream.create_stream
R2 [6]: Co-locate the PARDOSA_REGISTRY KV bucket on the same JetStream
  cluster as the streams it references so registry CAS and stream
  publish share a failure domain
R3 [5]: Verify the connected nats-server version against
  pardosa::nats::MINIMUM_SERVER_VERSION at startup and refuse to
  begin admitting writes when the running server is older
R4 [6]: Record the cluster name and replica count in the registry
  value so consumers can detect topology drift across generations

## Consequences

Topology assumptions become testable preconditions rather than
folklore. Operators receive a clear startup error when the cluster is
misconfigured. Trade-offs: pardosa cannot run against single-node
JetStream in production; development environments need a local
three-replica cluster or an explicit override flag. Upgrading
nats-server requires a coordinated bump of MINIMUM_SERVER_VERSION and
matching CI fixtures.
