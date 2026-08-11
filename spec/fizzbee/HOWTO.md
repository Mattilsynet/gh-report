# Working with the FizzBee specs

Hands-on guide for the design-level model-checking specs in this directory.
For the invariant ↔ code-seam ↔ ADR index and the observed results, see
[`README.md`](./README.md).

## What you have
- **`fizz`** CLI (0.5.1, via `brew install fizzbee`) — the model checker.
  Run: `fizz spec/fizzbee/<name>.fizz`.
- **`dot`** (graphviz) — renders the state graph fizz emits.
- **Seven specs** in this directory:
  - `occ_fence.fizz` — #1 N-writer OCC fence + re-arm
  - `budget_gate.fizz` — #2 budget-gate election/epoch (the F1 model)
  - `ordering_single_flight.fizz` — #3 TOCTOU single-flight create
  - `token_bucket.fizz` — #4 clock-driven token bucket (F2 elimination)
  - `concurrent_writer_overlap.fizz` — #5 overlap classification (STPA DR-10)
  - `scheduler_cancel_responsiveness.fizz` — #6 scheduler cancel race (STPA CA1)
  - `drain_timeout_classification.fizz` — #7 shutdown-drain outcome (STPA CA6)
- **Reference docs** at `~/.claude/skills/fizzbee-docs/`
  (`LANGUAGE_REFERENCE.md`, `GOTCHAS.md`, `VERIFICATION_GUIDE.md`, `examples/`).

## 1. The core loop — run a spec
```bash
fizz spec/fizzbee/budget_gate.fizz
```
Read the output bottom-up:
```
Nodes: 81, queued: 0 ...           # states the checker explored (the whole reachable space)
Valid Nodes: 81 Unique states: 39  # deduped state count
Checking eventually always NoParkedWorkerStuckForever
IsLive: true                       # liveness properties hold
PASSED: Model checker completed successfully   # every `always` (safety) held too
```
`PASSED` = **every reachable interleaving** satisfied every safety (`always`)
and liveness (`eventually`) property. This is not sampling like a test — it is
exhaustive over the modeled state space.

## 2. See a spec *fail* — this is how you trust it
A spec that only ever passes is suspicious (it might assert nothing). Each
spec's `README.md` section documents a one-line "teeth" edit that must break
it. Do the #2 F1 reproduction yourself.

In `budget_gate.fizz`, find `AcquireFastPath` and replace the refund branch:
```python
    else:
        if calls > 0:          #  <- delete these
            calls = calls - 1  #  <- two lines
```
with:
```python
    else:
        pass
```
Re-run `fizz spec/fizzbee/budget_gate.fizz`:
```
FAILED: Model checker failed. Invariant:  CallsTracksOnlyRealSpend
... Any:outcome="free304"
state: {"calls":1,"real_spend":0, ...}
```
That is **FizzBee independently rediscovering the F1 bug** we fixed in prod:
one free 304 drives `calls > real_spend` -> spurious exhaustion. Then restore
the passing model:
```bash
git checkout spec/fizzbee/budget_gate.fizz
```
The equivalent teeth edits for #1 (retry-to-win -> liveness fail) and #3 (split
lookup/commit -> duplicate stream) are spelled out verbatim in `README.md`.

## 3. Read a counterexample trace
When a spec FAILs, fizz prints the shortest path to the violation — `Init` then
each action with a state snapshot:
```
Worker#0.AcquireFastPath -> Any:outcome="free304" -> state:{calls:1, real_spend:0}
```
Read it top-to-bottom as "the sequence of steps that reaches the bad state,"
then map each action back to the real code seam named in the spec's header
comments (e.g. `AcquireFastPath` -> `budget.rs:acquire() :142-199`). That trace
*is* the bug report.

## 4. Visualize the state graph
Every run writes a dotfile. Render it:
```bash
fizz spec/fizzbee/occ_fence.fizz
dot -Tsvg spec/fizzbee/out/run_*/graph.dot -o /tmp/g.svg && open /tmp/g.svg
```
Green borders/arrows = live nodes / fair transitions. Useful for *seeing* how
writers interleave in #1. (`out/` is gitignored — never commit it.)

## 5. Anatomy of a spec (so you can edit them)
Bodies are Starlark (a Python subset). The FizzBee-specific pieces, all present
in these specs:

| Construct | Meaning |
|---|---|
| `action Init:` | one-time initial state |
| `atomic action Foo:` | a step executed indivisibly |
| `role Worker:` … `bag()` | multiple concurrent actors (the racers) |
| `any x in [...]` / `oneof:` | nondeterminism — checker explores **every** choice (exists) |
| `require <cond>` | guard: the action is only enabled when true |
| `always assertion X: return …` | **safety** — must hold in every state |
| `eventually always` / `always eventually` | **liveness** — must eventually (stay) true |
| `fair` / `fair<strong>` | fairness — the action must eventually run (needed for liveness) |

Full syntax + gotchas: `~/.claude/skills/fizzbee-docs/LANGUAGE_REFERENCE.md`.

## 6. Tune model size vs. coverage
Each spec has constants near the top: `NUM_WORKERS`, `LIMIT`, `EPOCHS` (#2);
`NUM_WRITERS` (#1); `NUM_DISPATCHES` (#3). Bump them to widen coverage — but the
state space grows fast (combinatorial). If a run gets slow, lower them or read
`PERFORMANCE_GUIDE.md` (symmetry reduction is the main lever). Global knobs go
in optional YAML front-matter at the top of the spec (`max_actions`,
`max_concurrent_actions`, `deadlock_detection`) — `budget_gate.fizz` already
sets `deadlock_detection: false` because its elector/park design has intended
terminal states.

## 7. Zero-install experimentation
Paste any spec into <https://fizzbee.io/play> to run it in the browser with an
interactive state graph — handy for quick "what if" edits before touching the
file.

## When to touch these specs again
- **A modeled design changes** -> update the spec and re-run. If you alter the
  fence/re-arm policy, the budget gate, or the merger create-path, the spec is
  the first place to prove the new design still holds — *before* writing Rust.
- **Keep the fidelity line straight:** these verify the **model**, not the Rust.
  The existing proptests (`live_nats_n_writer_fence_property.rs`,
  `i1_toctou_pin.rs`) remain the implementation-fidelity check. The model proves
  the *design*; the proptests prove the *code matches it*.

## CI wiring (PGN-0026)
`.github/workflows/ci-reusable.yml`'s `fizzbee-corpus` job installs a
pinned, checksum-verified `fizz` (`tools/.fizzbee-version`) and runs every
`spec/fizzbee/*.fizz`, failing the build on any non-`PASSED` result. This
is a design-level gate (PGN-0026:R1) — it does NOT discharge PGN-0021:R1's
OCC-fence obligation (PGN-0026:R2/R3).
