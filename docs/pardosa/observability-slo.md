# Pardosa observability: consumer advisory

Status: advisory, not a ratified ADR, deployed alerting configuration or SLO
guarantee. This note retains consumer design targets from the Phase-1 roadmap;
it does not own Pardosa implementation policy.

## Upstream ownership

The dependency revisions are owned by the workspace `Cargo.toml` and
`Cargo.lock`. Normative storage and ownership requirements belong to the
upstream [Pardosa specification](https://github.com/acje/pardosa/blob/08fcd290694553baa7fd5c3408bc18e9bd5eafb5/docs/spec/pardosa-1.0.md).
Backend ownership guidance belongs to the upstream
[`pardosa-nats` README](https://github.com/acje/pardosa/blob/08fcd290694553baa7fd5c3408bc18e9bd5eafb5/crates/pardosa-nats/README.md):
one writer per artefact is externally enforced; metadata epoch and data CAS
are separate, not atomic ownership-and-write fencing. Concurrent takeover
requires external coordination. Consumer documentation must not substitute
stronger ownership guarantees or local implementation paths.

## Emission is not detection

Structured metric/log emission alone is **emit-only**, not **detect**.
Detection requires a deployed aggregation rule, a tested alert condition and
an on-call route. None is established by this document. A log-to-metric
pipeline remains a reversible operational option; no instrumentation dependency
or pipeline deployment is introduced here.

## Suggested aggregation targets

| Signal | Advisory interpretation |
|---|---|
| Fence conflicts | Transient contention can mean fencing works; investigate spikes or unexpected multiple writers, not every conflict as bypass. |
| Conflict surfaced to caller | Does not prove the caller failed to abort/re-drive; that stronger signal needs call-site instrumentation. |
| Bridge duration / timeout | Compare latency and sustained timeouts with an explicitly chosen deployment budget. |
| Replay lag | Investigate growing, non-converging read-side staleness. |
| Dedup hits | Retry observability, not an independent duplicate-append correctness guarantee. |

Upstream source and tests own which signals exist and their boundary semantics;
this note does not attest current emission, cardinality, self-fence
unreachability or redelivery coverage. Before operational adoption, measure
label cardinality and choose concrete budgets and alert windows. Promoting
these suggestions to binding deploy gates or SLA guarantees needs a separate
ratified operational decision, not a stronger reading of this advisory.
