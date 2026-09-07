# Offline observation decision prototype

Under ghr-p3jrc.32; preparation for ghr-1crru, not its production fix.
Run `python3.12 -S -B tools/test_observation_prototype.py` from gh-report.
No dependencies, threads, sleeps, network, history, background heartbeat,
monitor wiring, schema changes, health verdicts, or restart actions.
Nine independent test methods run bounded loops (maximum 120 iterations).
Two additional constructor/observer-clock regressions live in
`test_team_match_prototype.py` to keep the review correction within five files.
Combined discovery runs 26 methods (17 team-module, 9 observation-module).
Memory is a fixed number of samples per test, not a production resource bound.

## Semantics exercised

`IdleUntil(deadline)` declares the original scheduler deadline. Reading it
cannot reset that deadline or dispatch. Explicit scheduler reevaluation can
dispatch when due; cancellation wins even at the deadline and Stopped cannot
dispatch again. This is an offline transition model, not a new polling cadence.

`WaitingUntil(deadline, reason)` declares a specific operation's wait, not
evidence of the underlying worker's continued progress. Expiry alone does not
mean completion. `Running(last_progress)` reports age without deciding that
age is unhealthy. Only an explicit worker event replaces its sample. Unknown
remains Unknown; Stopped means an observed terminal transition, not success.
Creation of Unknown models missing initial evidence, not a real restart test.

Separate samples represent batch supervisor and actual worker: two hours
awaiting a batch can coexist with two hours of unchanging worker progress.
Sibling updates and status reads cannot hide this. Samples are frozen Python
values, not Rust ownership proofs or a production concurrency implementation.
Instants measure nonnegative elapsed time from the fake-clock origin; frozen
dataclass construction and replacement reject negative elapsed values with
`ValueError`. `after` and clock advancement reject negative durations. An
observer clock before Running.last_progress raises explicit `ValueError`, never
a negative ProgressAge; equal instants produce zero age. These are runtime
boundary checks, not protection against deliberate Python reflection bypasses
or a general untrusted DTO validator. Wait outputs retain their declared reason,
with idle mapped to IDLE. No wall clock is consumed. Cancellation in the model means observed
idle cancellation/terminal acknowledgement, NOT that requesting cancellation
has already stopped in-flight production work.

Cases: one-hour idle; quota fallback one hour; authoritative reset one hour
plus five seconds and maximum accepted distance one day plus five seconds;
two-hour batch; caller-supplied Retry-After (three-hour synthetic example, not
a production maximum); fence backoff (two-second synthetic example); exact
deadline and one microsecond beyond; cancellation; missing evidence; sibling
isolation. These examples do not choose warning/critical thresholds.

## Production event sites and missing owners

Paths below are relative to `crates/gh-report/src/`, read 2026-09-06.

| Transition supplier | Exact source | Missing connection / owner |
| --- | --- | --- |
| Idle declaration and due/cancel decision | `app/daemon.rs:100-114,762-775` | Collection scheduler must publish the single original monotonic deadline when entering sleep; no shared observation sample exists here. |
| Initial/scheduled run entry and outcome | `app/daemon.rs:726-759,775-815` | Scheduler owns run lifecycle, not inner worker progress. Completed, cancelled, fenced, and error must retain distinct outcome meaning. |
| Phase and batch-result progress | `app/collect.rs:793-822,846,906,926-965` | Sweep saga can emit actual transitions; Pending polls cannot. `emit_progress` at 1056-1064 currently logs only. |
| Declared two-hour batch wait | `app/collect.rs:922-953`; `config/mod.rs:313` | Saga owns batch deadline; publish the SAME instant used for its timer. Not a bound on inventory, render/publish, fence retries, or total run. Individual worker completion/stall needs worker-owned signal, not batch supervisor age. |
| Finalization and actual visible commit | `app/collect.rs:1006-1037` | Saga/report publisher can distinguish render result from cache commit; awaiting finalization is not progress. |
| Quota wait begin/end/cancel | `github/replenish.rs:82-106,179-190`; `config/mod.rs:233` | Replenish policy knows duration/source and cancel result; needs component/run association and an observation sink. Actual authoritative reset may be 24h+5s, correcting bbobn's 1h upper-bound shorthand. |
| Secondary-limit deadline and actual wait | `github/rate_limit.rs:71-112`; `github/client.rs:507-512,764-775,805-813` | Client records resume instant then sleeps computed backoff; request owner must publish begin/end tied to its worker. Parser observation alone is not worker progress. Generic admission owner also waits at `crates/cherry-pit-wq/src/backoff.rs:84-100` (workspace-relative), with cancellation and possible authoritative extension. Neither path publishes this observation model. |
| Fence retry wait and attempt outcomes | `app/daemon.rs:487-514` | Convergence sink owns backoff duration, resync and retry outcomes. Its awaited operations lack a whole-operation deadline here. Do not invent one. |
| Sibling team refresh / client polling | `app/daemon.rs:857-920` | Separate team-loop owner; no authority to advance collection-worker state. Client availability polling has no overall deadline here; do not label it bounded WaitingUntil without one. |
| Stopped versus request-to-stop | `app/daemon.rs:769-772`; quota cancel `github/replenish.rs:94` | Loop terminal return / task supervision must acknowledge stop. Missing supervision-to-observation wiring for panic/abort; last sample alone cannot distinguish them from stall. |

Existing status/health gap is recorded in ghr-p3jrc.28: status reads metadata,
and healthz is static. This prototype adds no status field or endpoint. A
future consumer also needs sample provenance, run/component identity, atomic
publication, restart-to-Unknown semantics, and observer availability handling.
Unobserved or failed reads must not be rendered as healthy. No independent
heartbeat may substitute for the worker-owned sample.

## Minimal operator choices, after tested cases

1. Select warning grace `W` and critical grace `C`, with `0 < W < C`, and
   observation delivery/detection target. For a declared wait ending at `D`,
   candidates are warning after `D + W`, critical after `D + C`, never before
   or at `D`. They apply to overdue time, not time since last worker progress.
   No numerical pair is recommended by these tests. Running progress age has
   no threshold until an operation-specific progress expectation is accepted;
   the two-hour batch deadline is not permission to hide a stalled worker.
2. Name the responsible operator, destination and rehearsed warning/critical
   response plus ADR revisit event (FLO-0014:R1-R3, lines 19-23). Unknown needs
   an explicit unavailable-observation response, not a healthy default.
   Restart policy remains unselected; this prototype makes no restart
   recommendation and changes no platform probe.

Implementers can continue tracing per-worker completion/aggregation and designing
component-owned publication without operator permission for a numeric policy.
Production wiring and end-to-end observer tests remain unimplemented. Existing
freshness multiplier/text are untouched. FLO-0014 is not discharged by this
prototype; parameterized threshold algebra above is a candidate, not a gate.
