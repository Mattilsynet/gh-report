# OCC fence model — source preparation, NOT verified

Prepared under `ghr-p3jrc.32`; original obligation `ghr-5e5b7b20` remains OPEN.
The standalone manifest and lock are unchanged, Stateright pinned to 0.31.0.
There is no production dependency edge. `src/model.rs` contains a test-only
Stateright `Model`, bounded types, transition relation, properties and seven
tests. **All Rust tests are written but UNEXECUTED; no type-check, compile,
checker exploration, counterexample run, safety proof or liveness proof has
occurred.** Formatting is not semantic verification. No red/green claim.

## Exact execution blocker

`ghr-z90n6` supersedes the old partial-intake checkpoint: all 17 inventoried
build-time boundary packages were source-audited, including 13 build scripts,
two proc-macro crates, version_check and Stateright itself. Compiler probes
were audited as benign; the audit found **no tool denial**. T2 remains
Indeterminate where publisher/yank/owner-history telemetry is absent, not Clear.
The active rust-dep-intake skill's unconditional process-spawn hard-stop still
conflicts with its benign compiler-probe guidance. This policy tension is NOT
malware evidence and has NOT been waived. Do not compile dependencies.

Next executable command, ONLY after commander resolution of that gate against
this exact lock and authorization of execution, from the gh-report root:

```sh
CARGO_TERM_PROGRESS_WHEN=never cargo test --manifest-path tools/occ-fence-model/Cargo.toml --locked --offline --lib -- --test-threads=1 --nocapture
```

Offline is reproducibility, NOT containment. Cache absence is another blocker,
not permission to re-resolve. After tests, run scoped clippy with `--all-targets`
and `-D warnings`, then obtain adversarial review. A killed/timed-out exploration
is incomplete, never proof. No model runner or network explorer is started here.
The library is test-only: plain `cargo build` and clippy without `--all-targets`
are VACUOUS for model validation. This standalone workspace does not inherit
the parent lint bar: `-- -D warnings` is mandatory on every future clippy run.

## Finite domain and state transitions

- Exactly two configurations: 2 and 3 distinct writers, one subject, initial
  empty log. `Seq::{Zero,One,Two,Three}` has no overflow/wrap constructor.
- Each writer has at most two runs (`Initial`, `Recovery`), at most three sends
  per run, and one gate-held broker request at a time. Request identity is its
  index in the immutable send-time request ledger, independent of expected,
  writer and run. Distinct sends may carry identical request contents.
- `Send` captures expected; `Broker` compares against the authoritative tip and
  appends ONE record atomically on equality. Mismatch emits Conflict, no append.
- `CompareAndSwap` is the normal mode; explicit faulty `BlindAppend` ignores
  the comparison. The invariant consults the independent send-time ledger,
  not an expected field manufactured on the appended record.
- `Deliver` releases the gate (`Phase::Ready`) and queues a pending local
  update. The gate phase is separate from local sequence and pending updates.
  Another same-handle `Send` can capture the stale local sequence before
  `Update`, including when `Update` runs before that new request reaches Broker.
  Updates use monotonic max, matching production. Pending updates are bounded
  by three commits and applied FIFO; arbitrary update completion order is NOT
  MODELLED. Crash discards pending local updates, but never durable commits.
  Delays are interleavings, not a timed network or explicit infinite stutter.
- Crash before send appends nothing. Crash after send does NOT cancel the
  request: `CrashedSent` can still commit at the broker. Crash after commit and
  loss of its acknowledgment retain the authoritative record.
- Lost ack means terminal uncertainty for that run, not success or conflict.
  It never rolls back a commit. Conflict, crash and ack loss cannot `Send`.
- Only `ReplayNewRun`, after `Stopped`, may reset expected to the authoritative
  tip, consuming the sole recovery allowance. There is NO in-band resync/retry.
  Recovery stands for newly established ownership plus authoritative replay;
  ownership acquisition is assumed, NOT an implemented election/lease protocol.
- A writer's OWN broker request resolves before its replay; its pending local
  updates must settle or be discarded by crash. Other writers' in-flight
  requests DO race replay; CAS rejects those whose captured expected is stale.
- At sequence 3 no new matching request can be sent; stale pending requests
  can still be rejected and acknowledgments/crashes/recovery can settle.
  Capacity is a model boundary, NOT a production overload policy.

Private types constrain external construction; defining-module code can still
construct inconsistent histories. No claim that all State invariants are encoded
in Rust types. The deliberate corruption test exercises this distinction.

## Properties and exploration limits

Protocol predicates check CAS predecessor/position consistency, unique request
appends and receipt-to-durable-record correspondence. Log capacity is an encoding
sanity assertion, NOT a protocol result. Sometimes
predicates demand witnesses for reaching capacity, conflict, lost committed ack
and a recovery commit. These are safety/coverage checks, NOT liveness properties.
Tests separately encode stale rejection/no retry, pre/postcommit crash, ack loss,
and same-handle send in the split delivery/update window. Corrupt-state tests
include pristine and committed positive controls before negative mutations.
`blind_append_has_an_actual_reachable_cas_counterexample` authors a legal
Send/Send/Broker/Broker trace in both modes: CAS must pass and BlindAppend must
violate history, with the second request expecting Zero but committing Two.
This is a transition-generated counterexample expectation, not a hand-corrupted
record or expected panic. It is UNEXECUTED, not observed mutant detection or
plant/fail/revert/clean evidence. It does not test BFS counterexample reporting.
Uniqueness and receipt predicates remain encoding checks without independent
faulty-transition mutants; they do not prove transport deduplication.

`exhaustive_bounded_safety` requests single-thread BFS plus `join`, `is_done`
and `assert_properties`, without depth/state/time cutoffs or symmetry reduction.
Passing requires all safety predicates and sometimes witnesses. No reachable
state counts or completion evidence are available yet. Stateright uses 64-bit
state fingerprints (`src/lib.rs:343–351`); hash collisions remain a tool-level
limitation, so even a completed run is not collision-free mathematical proof.

Application-state bounds: ≤3 log records, ≤3 writers, ≤18 request-ledger entries,
one gate-held request per writer, ≤3 total pending updates, ≤1 recovery per writer.
The send bound is an exploration limit, not production admission policy.
Exploration retains the reachable graph;
no measured byte/state-count limit or aggregate process-memory bound is claimed.
One checker worker at a time; no model network I/O, background service or retries.
The future execution budget must bound wall time externally; interruption means
Unknown/incomplete, and no finite completion runtime is asserted here.
Enabledness currently clones the state for each candidate action (eight per
writer) and the checker recomputes accepted transitions; no allocation-cost
optimization or measured memory improvement is claimed.

### Fairness and liveness boundary

No fairness filter and no `Property::eventually` are installed. For conditional
progress one would need a nonfailed eligible writer, room below sequence 3,
eventual broker scheduling, eventual delivery of a nonlost ack and local update,
plus eventual authoritative ownership/replay after an aborted run. Permanent
loss/crash need not satisfy those premises. The two-run bound prevents repeated
crash/recovery and capacity prevents unbounded useful progress. Finite terminal
paths cannot establish weak/strong fairness over infinite executions. Stateright
0.31.0 explicitly warns that `eventually` ignores cycle-closing failures
(`src/lib.rs:284–299`). Fair liveness needs a separately justified encoding/tool
and proof; it is not silently inferred from acyclic bounded exploration.

## Production mapping and fidelity obligations

Mapping read from the current dirty tree; not clean-commit equivalence.

| Model surface | Current source / binding rule | Fidelity limit |
| --- | --- | --- |
| Ready → Send expected capture | `crates/pardosa-nats/src/handle.rs:222–243,762–789`; PGN-0016:R1 | Single subject, dense abstract sequence; real stream positions may have gaps. |
| Per-writer gate phase plus pending local updates | `handle.rs:223–227` append gate | One gate-held request; up to three sends/run; pending updates FIFO only. |
| Broker compare+append | PGN-0016:R2,R4 | Assumes broker CAS/append atomicity; does not prove JetStream/Raft. |
| Awaiting → gate Ready + pending update | `handle.rs:687–711,246–249,769–773` | Same-handle stale send after release/before update is modelled; monotonic local update can interleave with the next request. No exhaustive production-concurrency equivalence claim. |
| Conflict → Stopped | `handle.rs:713–743`; PGN-0016:R2,R9,R10 | Typed mismatch abstraction, not executable error-code adapter coverage. |
| ReplayNewRun | PGN-0016:R2,R7,R10; PGN-0021:R2 | New-run ownership/replay assumption; no adapter/shutdown or delayed old-run recovery proof. |
| CAS history and uniqueness | PGN-0016:R11; PGN-0021:R1,R6 | No transport redelivery/dedup window, payload semantics or whole-app idempotency proof. |

Read Stateright 0.31.0 source API before authoring: `src/lib.rs:158–179`
(`Model`), `232–259` (properties/checker), `267–310` (property constructors),
`src/checker.rs:153–168,268–275,298–349,477–529` (BFS, threads, join, assertions).
This is source-level API alignment, not successful compilation.

Excludes partitions, multi-subject traffic, arbitrary restarts, payload integrity,
duplicate transport requests, multi-event batch atomicity and production recovery
policy/asymmetry. PGN-0021's proptest-extension authoring layer is still absent
and needs separate intake; this preparation does not replace that requirement.
Keep `ghr-5e5b7b20`, formal review and executable acceptance open. No schema,
manifest, lock, production source, unrelated artifacts, commit or push changes.
