# CHE-0101. Dual-Queue Strangler Cutover Protocol for cherry-pit-wq

Date: 2026-07-30
Last-reviewed: 2026-07-30

Tier: B
Status: Accepted
Crates: cherry-pit-wq

## Related

References: CHE-0055, COM-0018, CHE-0006, CHE-0061, CHE-0041, CHE-0033, CHE-0034, PGN-0016, CHE-0039, CHE-0024, CHE-0046

## Context

Increment A (mission `cmdqueue-incrementA`, oracle survey `adr-fmt-djfws`)
adds a composable `Regulator` seam and an additive
`run_worker_pool_regulated` alongside the frozen `run_worker_pool`
(CHE-0055:R8 / CHE-0022:R1 additive-only floor). Running two worker pools
in parallel during a strangler-fig migration raises a gap no existing
ADR closes: the single-writer-per-aggregate ADRs (COM-0018, CHE-0006,
CHE-0061) forbid overlapping writers but none prescribes the
ownership-transfer protocol for a period when two independently-runnable
pools are both live. Sibling precedent exists at PGN-0025 ("Schema
Migration Cutover Topologies") for pardosa's schema-cutover problem, but
that ADR governs event-log schema versions, not wq command-queue
topology — this ADR is the wq-domain counterpart, scoped to which pool
may enqueue which `DomainKey`, not to storage schema.

## Decision

Ratify the enqueue-partition invariant and the associated cutover
protocol for running `run_worker_pool` (old) and
`run_worker_pool_regulated` (new) in parallel during a strangler-fig
migration of `cherry-pit-wq` consumers.

R1 [5]: ENQUEUE-PARTITION INVARIANT (hardest gate) — at any instant no
  `DomainKey` (aggregate) is enqueued in both the old (`run_worker_pool`)
  and new (`run_worker_pool_regulated`) pool. Ownership is partitioned
  per aggregate; the partition set is explicit, tracked by the consumer,
  never implied by wiring (COM-0018:R2). A both-queues-dispatch-freely
  design that lets any `DomainKey` reach either pool is a design-error
  class violation, not a lockable race, and is rejected outright.

R2 [5]: OLD PATH FROZEN — `run_worker_pool` and the `BudgetGate` /
  `RateLimitState` surface it consumes are frozen and run unchanged
  alongside the new pool. The new pool is a new additive type, never a
  redefinition of the old (CHE-0055:R8 / CHE-0022:R1). Cutover never
  reshapes the frozen surface.

R3 [5]: ADDITIVE NEW FUNCTION ONLY — the new path is reached solely via
  `run_worker_pool_regulated`; the old function is not rewired to
  delegate to it. Both are independently runnable, independently
  testable, and independently revertible.

R4 [5]: FENCED HAND-OFF OR PARTITION-SCHEDULE — an aggregate moves from
  the old pool to the new pool only via (a) a partition-schedule cutover
  (the aggregate is drained from the old pool before being enqueued to
  the new pool — no overlap window), or (b) a fenced ownership transfer
  (COM-0018:R5 fence/lease/epoch; `expected_sequence` CHE-0041:R3 at the
  pgno level, PGN-0016 subject-sequence fence at the NATS level) before
  the new pool writes an aggregate the old pool wrote. Independent
  sequence allocation across queues is forbidden — both go through the
  same `expected_sequence` authority (CHE-0033:R3 / CHE-0034:R3: sequence
  is the sole ordering authority).

R5 [5]: PER-INCREMENT VERIFICATION ALONGSIDE THE OLD QUEUE — each
  increment that touches the dual-pool arrangement is verified with both
  queues live: a test asserts the enqueue-partition invariant (R1) holds
  (no `DomainKey` in both pools) and that a job routed to the new pool
  produces the same `JobOutcome` shape plus threaded `CorrelationContext`
  (CHE-0039 / CHE-0055:R4 / CHE-0055:R6) as the old pool. Correlation and
  persist-then-publish ordering (CHE-0024:R1) stay identical across pools
  so `EventBus` dedup (CHE-0046:R4) remains safe.

R6 [5]: REVERSIBLE ROLLBACK — cutover is reversible per aggregate: an
  aggregate can move back from the new pool to the old pool by the same
  drain-or-fence protocol (R4). No cutover step is destructive or
  irreversible.

## Consequences

**Positive.** The strangler-fig migration has an explicit, ratified
protocol rather than an implied one; R1's partition invariant is
citable and testable independently of any single increment; R5 makes
"both queues live" a standing verification requirement rather than a
one-off check; R6 keeps every step reversible, bounding migration risk.

**Negative.** Consumers running the dual-pool arrangement must track
the partition set explicitly (R1) — this is new bookkeeping the
single-pool world did not require. Fenced hand-off (R4) requires
`expected_sequence` coordination across both pools, adding a dependency
on the pgno/NATS sequencing surfaces during the migration window.

**Open / deferred.** The concrete Retry-After / secondary-limit
`Regulator` (F3) and the clock-driven token-bucket `Regulator`
(increment B) are separate, later increments — this ADR governs the
cutover topology, not those regulators' internals. The consumer-side
partition-schedule mechanism (how a specific consumer enumerates and
persists its partition set) is left to the consuming crate; this ADR
ratifies the invariant the mechanism must uphold, not the mechanism
itself.

## Rejected Alternatives

**Both-queues-dispatch-freely (no partition).** Let any worker enqueue
any `DomainKey` to either pool based on availability or load. Rejected:
this is exactly the design-error class R1 forbids — it reintroduces
overlapping writers per aggregate, which COM-0018/CHE-0006/CHE-0061
already forbid at the single-pool level, and no fencing mechanism can
retrofit safety onto unpartitioned dispatch after the fact.

**Rewire `run_worker_pool` to delegate to `run_worker_pool_regulated`.**
Would collapse the two entry points into one, defeating the point of an
additive strangler-fig seam and risking a silent behavioural change on
every existing caller of the frozen function. Rejected per R2/R3 and
per the Increment A mission's explicit SemVer-major fence (CHE-0055:R8)
against reshaping the frozen surface.

**Independent sequence counters per pool.** Would have let the new pool
allocate its own `expected_sequence` stream, decoupled from the old
pool's. Rejected: this breaks CHE-0033:R3 / CHE-0034:R3's single
ordering authority and reopens the exact overlapping-writer hazard R1
exists to prevent — two independent counters cannot detect a
concurrent write to the same aggregate across pools.
