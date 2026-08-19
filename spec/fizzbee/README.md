# FizzBee specs — design-level model checking for three concurrency invariants

These specs model the **design** of three concurrency/ordering invariants
in this workspace, not the Rust implementation byte-for-byte. Value: a
design-level proof (the model's state space is fully explored and the
safety/liveness properties hold) plus a living, falsifiable spec.
**Fidelity caveat**: the existing Rust proptests
(`live_nats_n_writer_fence_property.rs`, `i1_toctou_pin.rs`) remain the
impl-fidelity check — these `.fizz` specs do NOT claim the Rust is
verified; they claim the MODEL is. Run any spec:

```
fizz spec/fizzbee/<name>.fizz
```

`out/` (fizz's graph/AST output) is gitignored — do not commit it.

---

## #1 — `occ_fence.fizz`: N-writer OCC fence + clean-abort re-arm

**Invariant.** N writers race to append to one aggregate's log at a
sequence via optimistic concurrency control (authoritative read, then
CAS-style commit). A loser surfaces a fence conflict, aborts cleanly (no
retry-to-win, per the `341188b` amendment), and re-arms with a fresh
authoritative read on the next attempt — this retry loop must eventually
let every writer commit (liveness), and no two writers may ever commit
the same sequence position (safety).

**Code seam.**
- `pardosa/src/authoritative.rs` — authoritative read of `committed_seq`
- pardosa-nats append gate (`Semaphore(1)`) — single-handle append
  serialization, `handle.rs:203-208`
- `crates/gh-report/src/app/daemon.rs::rearm_fenced_run` (PGN-0016:R7) —
  on `FencedConflict`/`ConcurrencyConflict`, abort cleanly and re-arm
  with a fresh read on the next scheduled tick
- Prose property pinned at
  `crates/gh-report/tests/live_nats_n_writer_fence_property.rs:1-16`

**ADRs.** PGN-0016 (fence + re-arm policy), PGN-0010 / PGN-0008 / PGN-0015
(synchronous public-facade boundary the OCC read/append sits behind).

**Command.** `fizz spec/fizzbee/occ_fence.fizz`

**Observed result (PASSED).**
```
Valid Nodes: 65 Unique states: 65
Checking eventually always EveryWriterEventuallyCommits
IsLive: true
PASSED: Model checker completed successfully
```
(NUM_WRITERS=3; retries are unbounded in the model — no numeric retry
counter — the state space stays finite because each writer commits at
most once and `committed_seq` is capped at NUM_WRITERS.)

**Teeth.** Change `AttemptAppend`'s conflict branch from "abort cleanly,
reset `read_seq` to -1" to "retry-to-win" (`pass` — leave `read_seq`
stale instead of resetting it, i.e. do NOT take a fresh authoritative
read). Captured violation:
```
Checking eventually always EveryWriterEventuallyCommits
IsLive: false
FAILED: Liveness check failed
Invariant: EveryWriterEventuallyCommits
...
Writer#2.AttemptAppend
--
state: {"committed_seq":2,"log":["Writer0","Writer1"], ...}
Writer#0: fields(done = True, read_seq = -1)
Writer#1: fields(done = True, read_seq = -1)
Writer#2: fields(done = False, read_seq = 1)
```
Writer#2's `read_seq` is permanently stuck at 1 (stale — `committed_seq`
has since advanced to 2) because the conflict branch never re-arms with
a fresh read. It stutters forever on the same failing `AttemptAppend`,
self-fenced. This is exactly the liveness bug the `341188b` amendment
(no retry-to-win) prevents.

---

## #2 — `budget_gate.fizz`: election/epoch/cooldown CAS gate (THE F1 MONEY SHOT)

**Invariant.** N workers acquire call permits against a shared epoch
counter (`calls`) bounded by `limit`. When exhausted, exactly one worker
CAS-elects (`resetting`) to reset the epoch and wake parked workers via
`epoch_advanced`. Some acquires turn out to be free (GitHub 304
conditional revalidation) and must be refunded — a saturating
CAS-decrement that never underflows below 0. **The property that
matters**: `calls` must track only real upstream spend
(`CallsTracksOnlyRealSpend: calls <= real_spend`) — a free 304 that is
refunded nets to zero; if it is NOT refunded it inflates `calls` and
drives spurious exhaustion even though real work is tiny. This is
exactly commit `934060d`'s F1 bug: ~34x overcounting, 3 false-positive
collection freezes in 7 days.

**Code seam.** `crates/cherry-pit-wq/src/budget.rs`
- `acquire()` CAS-increment loop — `:142-199`
- `resetting` `AtomicBool` CAS election — `:160-164`
- `epoch_advanced` `Notify` (wake parked) — `:192-198`,
  `ResetGuard::drop` — `:246-250`
- cooldown reset (`calls := 0`) — `:187`
- `refund()` CAS-decrement, saturating at 0 — `:225-239` (the F1 fix,
  commit `934060d`)

**ADRs.** None dedicated — cherry-pit-wq internal concurrency primitive,
not an ADR-governed contract surface; cited here for the F1 production
incident history (commit `934060d`).

**Command.** `fizz spec/fizzbee/budget_gate.fizz`

**Observed result (PASSED, refund present).**
```
Valid Nodes: 81 Unique states: 39
Checking eventually always NoParkedWorkerStuckForever
IsLive: true
PASSED: Model checker completed successfully
```
(NUM_WORKERS=3, LIMIT=2, EPOCHS=2 — enough to exercise both the fast
acquire path and at least one full election/reset/wake cycle.)

**Teeth — THE MONEY SHOT (reproduces the F1 bug).** In `AcquireFastPath`,
delete the refund branch for the `free304` outcome — i.e. change:
```python
else:
    if calls > 0:
        calls = calls - 1
```
to:
```python
else:
    pass  # TEETH: PRE-F1 bug — no refund, free 304 still burns budget
```
Captured violation — `fizz` fails on the **very first acquire**:
```
FAILED: Model checker failed. Invariant:  CallsTracksOnlyRealSpend
------
Init
--
state: {"calls":0,"limit":2,"real_spend":0,"resetting":false,"total_epochs":0,"workers":[...]}
Worker#0: fields(done_this_epoch = False, state = "idle")
Worker#1: fields(done_this_epoch = False, state = "idle")
Worker#2: fields(done_this_epoch = False, state = "idle")
------
Worker#0.AcquireFastPath
--
state: {"calls":0,"limit":2,"real_spend":0, ...}
------
Any:outcome="free304"
--
state: {"calls":1,"limit":2,"real_spend":0,"resetting":false,"total_epochs":0, ...}
Worker#0: fields(done_this_epoch = True, state = "idle")
```
`calls:1, real_spend:0` — `calls > real_spend` the instant a single free
304 lands without a refund. At scale this is precisely the production
shape: a workload that is mostly free 304s still burns down the epoch
budget as if every request were real spend, driving `calls` to `limit`
and triggering the CAS-elect/cooldown/reset path (the `warn!("API
budget exhausted, pausing collection")` + `wait_duration` sleep) for
work that was never really consuming quota. Restoring the refund branch
(this repo's committed `budget_gate.fizz`) makes `fizz` PASS again.

---

## #3 — `ordering_single_flight.fizz`: TOCTOU single-flight create + ordering

**Invariant (the four i1 invariants).** N concurrent same-`domain_key`
dispatches against one merger: (1) exactly one routing-index entry
materialises; (2) the stream is monotonic `1..=N` (1 create + N-1
appends, no orphan stream, no gaps); (3) the bus log holds N envelopes
in sequence order; (4) the per-aggregate sequence tracker records N.
These hold **iff** the `lookup -> EventStore::create -> index.or_insert`
sequence is single-flighted (atomic/guarded) across concurrent
dispatchers.

**Code seam.**
- `crates/cherry-pit-merger/tests/i1_toctou_pin.rs:1-20` — the four
  pinned invariants this model restates as `always` assertions
- merger create-path `lookup -> EventStore::create -> index.or_insert`,
  cited at `repo_service.rs:493` per that test's header
- `cherry-pit-core/src/checkpoint.rs` (CHE-0097 checkpoint monotonicity)
- CHE-0019 (duplicate `event_id` rejection)

**ADRs.** CHE-0097 (checkpoint sequence monotonicity — modeled as a
`transition assertion`), CHE-0019 (duplicate-event rejection — modeled
as `NoDuplicateStreamPositions`).

**Command.** `fizz spec/fizzbee/ordering_single_flight.fizz`

**Observed result (PASSED).**
```
Valid Nodes: 4 Unique states: 4
Checking eventually always AllDispatchesEventuallyComplete
IsLive: true
PASSED: Model checker completed successfully
```
(NUM_DISPATCHES=3. The passing model's single atomic `Dispatch` action
covers the whole lookup..append cycle as ONE indivisible step — this is
the single-flight guard, made explicit. Because the guard is atomic,
there is no possible interleaving to explore: the state space is a
straight line of 4 states (Init + 3 dispatches), which is exactly the
expected shape for a correctly-serialized single-flight guard.)

**Teeth.** Drop the single-flight guard by splitting lookup from
create/append into two separate atomic actions per dispatcher —
`LookupPhase` (reads `routing_index`, decides `would_create`, but does
NOT mutate yet) and `CommitPhase` (does the actual create-or-append,
using the possibly-stale `would_create` decision). With 2 concurrent
`Dispatcher` roles, `fizz` finds the interleaving where both look up
before either commits:
```
FAILED: Model checker failed. Invariant:  NoDuplicateStreamPositions
------
Dispatcher#0.LookupPhase
--
state: {..., "routing_index":{}, "stream":[], "total_dispatched":1}
Dispatcher#0: fields(phase = "looked_up", would_create = True)
------
Dispatcher#1.LookupPhase
--
state: {..., "routing_index":{}, "stream":[], "total_dispatched":2}
Dispatcher#0: fields(phase = "looked_up", would_create = True)
Dispatcher#1: fields(phase = "looked_up", would_create = True)
------
Dispatcher#0.CommitPhase
--
state: {"bus_log":[1], "routing_index":{"AGG":"AGG"}, "seq_tracker":1, "stream":[1], ...}
------
Dispatcher#1.CommitPhase
--
state: {"bus_log":[1,1], "routing_index":{"AGG":"AGG"}, "seq_tracker":1, "stream":[1,1], ...}
Dispatcher#0: fields(phase = "idle", would_create = True)
Dispatcher#1: fields(phase = "idle", would_create = True)
```
Both dispatchers observed an empty `routing_index` during `LookupPhase`
before either ran `CommitPhase`, so both commit a seq-1 create:
`stream:[1,1]` — a duplicate stream position (would be an orphan second
aggregate for a keyed routing_index) and a gap where seq 2 should be.
`NoDuplicateStreamPositions` and `StreamMonotonicNoGaps` both fail —
exactly the TOCTOU regression `i1_toctou_pin.rs`'s single-flight guard
prevents. (The split-phase teeth variant is not committed as a separate
file — it is fully specified in `ordering_single_flight.fizz`'s trailing
comment block and was run ad hoc from that pseudocode to capture this
evidence; re-derive it verbatim from the comment to reproduce.)

---

## #4 — `token_bucket.fizz`: clock-driven token bucket (increment B — F2 elimination)

**Invariant.** N workers race to debit a token-bucket regulator whose
available-token count is a pure function of elapsed clock time —
`available(now) = min(capacity, generated(now) - consumed)`, where
`generated(now) = elapsed(now) * rate` grows without bound and
`consumed` is a single monotonic `AtomicU64`. A debit succeeds via one
lock-free CAS; no worker elects itself to reset shared state, and no
worker parks on a wakeup channel — this is the design change increment
B makes to eliminate `budget_gate.fizz`'s F2 shared-election state-space
explosion (the `resetting` `AtomicBool` election + `epoch_advanced`
`Notify` park/wake pair).

**Code seam.** `crates/cherry-pit-wq/src/token_bucket.rs`
- `TokenBucketRegulator::try_debit_one` — single-CAS debit loop
- `generated`/`available` computation — pure function of `Instant`,
  recomputed from scratch every call, no stored "last refill" state
- `impl Regulator for TokenBucketRegulator` — third additive impl
  behind the increment-A seam (`crates/cherry-pit-wq/src/regulator.rs`)

**ADRs.** CHE-0055 (Regulator seam, additive-only per CHE-0022:R1),
CHE-0007 (`#![forbid(unsafe_code)]` — lock-freedom via safe
`std::sync::atomic` only, no `unsafe`), CHE-0029 (hand-rolled
tokio-only, no new crate dependency).

**Command.** `fizz spec/fizzbee/token_bucket.fizz`

**Observed result (PASSED).**
```
Valid Nodes: 61 Unique states: 61
Checking eventually always AllWorkersEventuallyDone
IsLive: true
PASSED: Model checker completed successfully
```
(NUM_WORKERS=3, CAPACITY=2, TICKS=3.)

**Node-count comparison vs `budget_gate.fizz` (the F2 shape being
replaced).**

| Model | Valid nodes | Unique states | Election/wakeup state? |
|---|---|---|---|
| `budget_gate.fizz` (NUM_WORKERS=3, LIMIT=2, EPOCHS=2) | 81 | 39 | Yes — `resetting` `AtomicBool` CAS-election + `epoch_advanced` `Notify` park/wake; a `Worker.state` field with an explicit `"parked"` value |
| `token_bucket.fizz` (NUM_WORKERS=3, CAPACITY=2, TICKS=3) | 61 | 61 | No — no election flag, no wakeup channel, no `"parked"` state exists in the model at all |

Both `Valid Nodes` counts were measured this session by running `fizz`
directly (`budget_gate.fizz`'s 81-node baseline is **reproduced**, not
assumed — the mission brief's cited figure checks out against the
committed file as-is). The 61-node token-bucket model shows a ~25%
raw node-count reduction under comparably-scaled bounds — but the
qualitative collapse is the load-bearing claim: `budget_gate.fizz` has
three actions per worker (`AcquireFastPath` / `AcquireElectReset` /
`AcquireParkOnFull`) plus a `state` field threading `idle -> parked ->
idle`, all of which exist solely to coordinate the shared
election/wakeup; `token_bucket.fizz` has exactly one action per worker
(`AttemptDebit`) and no coordination-only state field at all. The
election/wakeup machinery is not shrunk — it is entirely absent, which
is what `AllWorkersEventuallyDone`'s liveness proof being structurally
trivial (no wakeup-fairness argument to make, unlike
`NoParkedWorkerStuckForever`) is evidence of.

**Teeth.** Not applicable in the F1-money-shot sense (there is no
refund/overcounting bug class here — RATE-only, no charge concept).
The falsifier for this model is the abort condition itself: if a
`resetting`-shaped election field or a `"parked"` `Worker.state` value
had to be reintroduced to make the model liveness-pass, that would
falsify the F2-elimination design premise. No such field exists in
`token_bucket.fizz` and the model passes both safety assertions
(`ConsumedNeverExceedsGenerated`, `ConsumedNeverExceedsCapacity`) and
the liveness assertion as committed.

**Atomicity assumption (linus review round 1, ghr-e2e72e32, Critical
finding).** `AttemptDebit` is declared `atomic`, collapsing the real
code's load-check-then-CAS into one indivisible model step. This is a
faithful abstraction only because `try_debit_one` reuses a single
`consumed_milli` load as both the availability check and the
`compare_exchange_weak` operand — a genuinely-atomic-effect retry loop
where a lost race fails the CAS and retries, rather than acting on a
stale decision. An earlier implementation revision took two independent
loads (one for the check, one for the CAS operand); that variant was
racy — two callers could both observe availability, then both CAS
against a post-race value and over-admit — and this model's `atomic`
action would NOT have been a faithful abstraction of it (the model's
own safety assertions would have passed while the real code violated
them, exactly the gap linus's review caught). This entry now assumes
the CAS-correct implementation, not the two-independent-loads variant.

---

## Follow-ups (out of scope for this mission)

- Wiring `fizz` into CI (noted as a possible follow-up bead; not
  implemented here).
- Model-based-testing Go adapters (`fizz-mbt`) — not this mission.
