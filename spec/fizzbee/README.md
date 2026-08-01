# FizzBee specs — design-level model checking for concurrency invariants and STPA UCAs

These specs model the **design** of concurrency/ordering invariants and
STPA-derived unsafe-control-actions (UCAs) in this workspace, not the Rust
implementation byte-for-byte. Value: a design-level proof (the model's state
space is fully explored and the safety/liveness properties hold) plus a
living, falsifiable spec.
**Fidelity caveat**: the existing Rust proptests
(`live_nats_n_writer_fence_property.rs`, `i1_toctou_pin.rs`) remain the
impl-fidelity check — these `.fizz` specs do NOT claim the Rust is
verified; they claim the MODEL is. Run any spec:

```
fizz spec/fizzbee/<name>.fizz
```

**Governance (PGN-0026).** FizzBee is adopted as the design-level / STPA-UCA
modeling tool, wired into CI as a gate (every spec must PASS). This is
explicitly NOT the PGN-0021:R1 OCC-fence exhaustive-checking obligation —
that remains assigned to Stateright (deferred, bead adr-fmt-2ysyq).
`occ_fence.fizz` and `concurrent_writer_overlap.fizz` both touch the
OCC-fence property but neither is the PGN-0021:R1 verification-of-record;
see PGN-0026:R2/R3.

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

**Atomicity assumption (linus review round 1, adr-fmt-tcg3b, Critical
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

## #5 — `concurrent_writer_overlap.fizz`: overlap classification (STPA DR-10)

**Invariant.** N writer instances overlap on one aggregate's OCC-fenced
append. Every instance that loses the race must be classified
(`WritePolicyCategory::Conflict`), never left unclassified — the formal
analogue of CHE-0088:R2's fail-closed `classify()` chokepoint. This is
DR-10's own framing verbatim: "a formal model should verify [overlap]
independently of runtime instrumentation."

**Code seam.**
- `crates/gh-report/src/app/write_policy.rs::WritePolicyCategory::classify`
  — `FencedConflict -> Conflict`; wildcard arm fails CLOSED to
  `Unrecoverable`, never silently open
- `crates/gh-report/src/app/write_policy.rs::WritePolicyCategory::response`
  — `Conflict -> Fatal`
- `crates/gh-report/src/app/daemon.rs::rearm_fenced_run` (PGN-0016:R7)

**ADRs.** PGN-0016, CHE-0088 (R2/R7/R8), PGN-0022 (NOT independently
verified — see Gap below).

**Gap (explicit, not fabricated).** PGN-0022:R1's structured detection
emission firing inside pardosa-nats was not located in gh-report's own
source this session (out of scope, per the STPA analysis's own flagged
Gap, bead adr-fmt-pq1b6.1.1). This model verifies gh-report's OWN
classification chokepoint never silently drops an overlap loss — it does
NOT claim to verify PGN-0022:R1's emission call-site exists or fires.

**Command.** `fizz spec/fizzbee/concurrent_writer_overlap.fizz`

**Observed result (PASSED).**
```
Valid Nodes: 131 Unique states: 131
Checking eventually always EveryWriterEventuallyResolved
IsLive: true
PASSED: Model checker completed successfully
```
(NUM_WRITERS=3.)

**Teeth.** In `AttemptAppend`'s conflict branch, drop the classification
(models a code path that forgets to route a `FencedConflict` through
`WritePolicyCategory::classify` — e.g. an early return before the
chokepoint):
```python
else:
    self.read_seq = -1   # category left at "none" — never set
```
Captured violation (re-run `fizz spec/fizzbee/concurrent_writer_overlap.fizz`
after the edit):
```
FAILED: Model checker failed. Invariant:  NoSilentOverlapLoss
------
Init
--
state: {"committed_seq":0,"log":[],"writers":[...]}
Writer#0: fields(attempted = False, category = "none", done = False, read_seq = -1)
```
`category = "none"` after an attempted writer's fenced conflict is exactly
the "swallowed failure" shape H5 names — a job outcome unknown, no
classification emitted. Restore with the committed file (or `git checkout`
if tracked).

---

## #6 — `scheduler_cancel_responsiveness.fizz`: cancel-vs-tick race (STPA CA1, DR-1/DR-9)

**Invariant.** Regardless of the previous collection cycle's outcome
(Completed, Cancelled, FencedConflict, Err), the scheduler loop's next-tick
gate always re-fires and always eventually honours a cancel request — no
outcome branch can special-case its way past the cancel check.

**Code seam.**
- `crates/gh-report/src/app/daemon.rs::next_collection_tick` (:119-132) —
  biased `select!` between `cancel.changed()` and `sleep(interval)`
- `crates/gh-report/src/app/daemon.rs::spawn_collection_loop` (:653-745) —
  every match arm on the collection outcome falls through to the top of
  the loop except `Cancel`

**ADRs.** None dedicated (a gap analogous to DR-7's — no ADR governs this
loop's cancel-check cadence either).

**What this model does NOT claim.** CA1-NP's genuine hazard (a panicked
task that never ticks again) is only detectable from OUTSIDE the loop
(DR-1's external staleness monitor) — this model verifies the narrower,
in-band claim: given the loop is alive, is cancel always honoured.

**Command.** `fizz spec/fizzbee/scheduler_cancel_responsiveness.fizz`

**Observed result (PASSED).**
```
Valid Nodes: 12 Unique states: 12
Checking eventually always CancelAlwaysEventuallyHonored
IsLive: true
PASSED: Model checker completed successfully
```
(MAX_TICKS=3.)

**Teeth.** Invert `NextTickCheck`'s guard (a plausible transcription bug
when refactoring the `match` arms):
```python
if cancel_requested:
    ticks_run = ticks_run + 1   # BUG: keeps ticking on cancel
else:
    loop_exited = True          # BUG: exits when nobody asked
```
Captured violation: `CancelAlwaysEventuallyHonored` fails (once
`cancel_requested` is True, the buggy branch keeps incrementing
`ticks_run` instead of setting `loop_exited`, so the assertion's
`return loop_exited` is never satisfied); `NoSpuriousExit` also fails
independently (the loop can exit while `cancel_requested` is still
False). Restore with the committed file.

---

## #7 — `drain_timeout_classification.fizz`: shutdown-drain outcome (STPA CA6, DR-7)

**Invariant.** Each of the three `drain_shutdown_with_timeout` phases
(worker-pool, delivery, collection) resolves to EXACTLY `drained` or
`timeout` once the shared budget elapses — never left unclassified
("pending" forever), and never classified `timeout` before the budget
actually expires.

**Code seam.**
- `crates/gh-report/src/app/daemon.rs:48` — `SHUTDOWN_DRAIN_TIMEOUT =
  Duration::from_secs(3)`
- `crates/gh-report/src/app/daemon.rs::drain_shutdown_with_timeout`
  (:290-335ish) — three phases, each logging `reason = "drained"` or
  `reason = "timeout"`; the collection phase's timeout case is the named
  `CollectionDrainError::Timeout`

**ADRs.** None dedicated — the STPA analysis itself flags this as a gap
("No dedicated ADR governs SHUTDOWN_DRAIN_TIMEOUT=3s"); this model
documents the classification SHAPE, not a claim that 3s is ADR-ratified.

**Command.** `fizz spec/fizzbee/drain_timeout_classification.fizz`

**Observed result (PASSED).**
```
Valid Nodes: 75 Unique states: 39
Checking eventually always EveryPhaseEventuallyResolved
IsLive: true
PASSED: Model checker completed successfully
```
(NUM_PHASES=3, DRAIN_BUDGET=3.)

**Teeth.** Remove `FinalizeAtBudget`'s classification (models forgetting
the `else` arm — the phase's join handle abandoned with no logged
outcome):
```python
atomic fair action FinalizeAtBudget:
    require elapsed == DRAIN_BUDGET
    pass   # phases left "pending" forever — no timeout classification
```
Captured violation: `EveryPhaseEventuallyResolved` fails — any phase
whose `MaybeFinish` never chose `ready = True` before the budget elapsed
stays "pending" forever, with no classification event a DR-7 counter
could observe. Restore with the committed file.

---

## STPA UCA coverage (bead adr-fmt-pq1b6.1.1, DR-1..DR-10)

| DR | UCA / hazard | Modeled? | Spec | Rationale |
|---|---|---|---|---|
| DR-1 | CA1-NP tick staleness | Partial | `scheduler_cancel_responsiveness.fizz` | In-band cancel-responsiveness modeled; the panicked-task/no-tick-at-all case is inherently only observable from an external monitor (STPA's own ICP note) — left to the runtime staleness monitor DR-1 specifies. |
| DR-2 | CA4-WT lock-TTL/interval coupling | No | — | A numeric-coupling tripwire (TTL vs observed run duration), not a control-flow/concurrency property a state-space model adds confidence over; better served by the runtime monitor DR-2 specifies. |
| DR-3 | CA3 lag-SLO ceiling | No (Gap) | — | STPA's own Step3 flagged this a Gap: the enforcement call-site and numeric ceiling are not visible in gh-report's source this session. Modeling an unconfirmed mechanism would be fabrication, not source-grounded modeling (abort_if branch) — left as a Gap, not modeled. |
| DR-4 | CA2-ST retry-ceiling severity | No | — | Already covered structurally by `write_policy.rs`'s closed-enum, no-wildcard `response()` dispatch (compile-time exhaustiveness); a log-level threshold monitor, not a concurrency model target. |
| DR-5/DR-6 | CA5 rate-limit/backoff | No | — | Single-process constant-threshold checks; existing budget_gate.fizz/token_bucket.fizz already cover the concurrency-bearing gate/regulator shape these constants feed. |
| DR-7 | CA6-ST drain-timeout classification | Yes | `drain_timeout_classification.fizz` | Modeled directly — a genuine concurrency/race property (work-completion vs. budget) a runtime counter alone can't prove has no unclassified-outcome path. |
| DR-8 | CA2 write-failure distribution | No | — | A rolling-window count monitor; the classification exhaustiveness it depends on is already structurally covered (see DR-4 note above and `concurrent_writer_overlap.fizz`). |
| DR-9 | CA1 team-refresh decoupling | Partial | `scheduler_cancel_responsiveness.fizz` | Same cancel-responsiveness shape as DR-1, decoupled cadence; the model is cadence-agnostic. |
| DR-10 | CA4 concurrent-writer-overlap | Yes | `concurrent_writer_overlap.fizz` | Modeled directly, per DR-10's own explicit naming of FizzBee as consumer — scoped to gh-report's classification chokepoint only (see Gap note in that spec's section above); PGN-0022's emission call-site remains unverified. |

---

## Follow-ups (out of scope for this mission)

- Model-based-testing Go adapters (`fizz-mbt`) — not this mission.
- Numeric ratification of DR-3's PGN-0023 lag-SLO ceiling, which would
  unblock modeling CA3 (currently a documented Gap, see coverage table
  above).
