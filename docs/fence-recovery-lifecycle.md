# Fence recovery lifecycle — evidence-qualified

Mission: `ghr-x2dys` (fence-lifecycle-20261005), child `ghr-x2dys.1`.
This document names lifecycle ownership and dispositions, distinguishing
**observed** behavior from **Unknown** evidence. It is **not an ADR**: it
ratifies no delegation (CHE-0088:R10 delegated semantics remain unratified,
ghr-jj7eq GAP-4) and edits no binding rule. Binding ADRs are cited where a
disposition is ADR-anchored; everything else is a code/test observation on
HEAD `955d1f21`.

## Ownership model (observed)

| Responsibility | Owner | Evidence |
|---|---|---|
| Append fence enforcement | pardosa JetStream single-writer fence rejects wrong-last-sequence appends by err_code, never description | docs/adr/pardosa/PGN-0016-*.md:31-34, 72-86 |
| Typed Conflict -> abort, no swallow | CHE-0088:R3; `Conflict` fatal | docs/adr/cherry/CHE-0088-*.md:28-42; write_policy.rs:86 |
| Detecting a fenced persist on delivery | `delivery_loop_with_recorder` -> `DeliveryStep::Fenced` | crates/gh-report/src/app/daemon.rs:1255, 1281 |
| Latched fence storage + batch release | `AppState::fence_active_run` (latch + `BatchTracker::drain`) | crates/gh-report/src/app/state/mod.rs:623-637 |
| Fence consumption at run boundary | `retire_batch` -> `batch_outcome` re-raises typed `FencedConflict` | crates/gh-report/src/app/collect.rs:1254-1273 |
| Sanctioned recovery sink | `converge_on_fence` (resync + bounded retry); `rearm_fenced_run` / `rearm_after_fenced_conflict` — no per-call-site re-arm (CHE-0088:R9); **exercised at helper level this increment — production wrappers `rearm_fenced_run` / `run_with_outcome` bypassed** | daemon.rs:519-594, 601-638; `rearm_cancelled_after_fence…` / `…recollection_is_idempotent` tests |
| Cross-run reown | abort run as `FencedConflict`, fresh authoritative resync, re-collect (PGN-0016:R2/R7) | daemon.rs:617; state/mod.rs:1976+ |
| Committed history | at-least-once; only uncommitted/below-threshold work may be shed | FLO-0005:R2; CHE-0041 R1/R2 |

## Delivery ownership by run state (observed)

| Run state | Disposition | Evidence |
|---|---|---|
| current (matching owner) | `ScheduledBatch` whose correlation == active owner id; success/failure steps accounted to the batch tracker | daemon.rs:1184-1194, 1256-1272 |
| stale (superseded / no matching owner) | outcome **discarded** with warning; the converged run re-collects it (PGN-0016:R2) | daemon.rs:1188-1193 |
| ownerless (`InitialLoad` / `External`) | steps applied with no run retirement; a fence there still latches the run | daemon.rs:1195-1196 |
| fenced run | delivery gate rejects further appends through the superseded writer | daemon.rs:1197-1204; state/mod.rs:642-647 |

## Interrupted-run disposition — observed (this increment)

Real-boundary fixture (not a `cfg(test)` accounting shim): a deterministic
schedule where repo-1's persist **commits before** repo-2's persist is
fenced-rejected, driven through worker pool -> `delivery_loop_with_recorder`
-> run fence -> saga boundary (real boundary for the fence and
committed-history path). The rearm recovery is then exercised at **helper
level**: `daemon::rearm_after_fenced_conflict` over `AppState::resync_event_store`
(the same resync `rearm_fenced_run` passes in production), invoked directly
from the test — the production wrappers `rearm_fenced_run` / `run_with_outcome`
are **bypassed**. The cancellation test cancels the token **before invoking
the helper** (pre-cancelled run leg); mid-backoff / mid-run cancellation is
**not** exercised and remains advisory. Tests (green, exit 0, this HEAD):
`committed_before_rejected_history_survives_and_recollection_is_idempotent`
and `rearm_cancelled_after_fence_preserves_committed_history_and_reports_cancelled`.

- **Committed-before-rejected history survives**: repo-1 remains in the
  projection after the run aborts `CollectionOutcome::FencedConflict`.
- **Rejected write is not accounted as a successful write**: the run reports
  `FencedConflict`, not `Completed` (asserted this increment); the fenced
  record's slot is released wholesale rather than completed individually —
  asserted by daemon.rs `fenced_record_aborts_the_batch_instead_of_completing_a_slot`
  (=3544), i.e. the slot-accounting claim is a read-level consequence cited
  to that test, not a new tracker assertion in this increment.
- **Recollection is idempotent through the rearm sink**: the production rearm
  re-collects repo-2 exactly once (attempt count 1 -> 2) and does not
  re-append repo-1 (attempt count stays 1), via same-day baseline reuse.
- **Committed history after real resync asserted at the store**: after the
  rearm's `resync_event_store`, `EventStoreImpl::events_with_fibers()` shows
  exactly one appended event for each of `id-repo-1` and `id-repo-2` — the
  discarded attempt was never appended, and replay did not duplicate.
- **Cancellation after the fence**: a doomed (pre-cancelled) rearm still runs
  the real resync, maps the run leg through the production cancel-mapping
  select, returns `CollectionOutcome::Cancelled` (the disposition
  `rearm_fenced_run` would log as "aborted on shutdown"), and preserves the
  real store's committed history — terminal disposition is observed, not
  deferred to Unknown.

## Cancellation / shutdown dispositions

| Disposition | Status | Evidence |
|---|---|---|
| `CancellationToken` fires during collection | **observed**: run reports `CollectionOutcome::Cancelled`, no report published | collect.rs:433-438 (select arm) |
| Delivery task on admission loss / outcome-channel close | **observed**: the delivery task exits (`break` on admission loss, exits on channel close). **Drain is not the exit path**: on the fence path `BatchTracker::drain` is performed by `fence_active_run`, not by the delivery exit | daemon.rs:1157-1159, 1180-1182, 1237 (exit); state/mod.rs:623-637 (drain) |
| Cancellation of the **pre-cancelled** rearm run leg (after the fence) | **observed (this increment, helper-level)**: token cancelled before helper invocation; resync still runs; run leg maps to `CollectionOutcome::Cancelled`; committed store history preserved. Mid-backoff / mid-run cancellation NOT covered (advisory) | `rearm_cancelled_after_fence_preserves_committed_history_and_reports_cancelled` |
| Cancellation mid-resync (inside `resync_from_authoritative`) | **Unknown** — the blocking resync is not cancellation-checked; no hook in that path | deferred |
| Cancellation before/after latch during the initial run | **Unknown** — the saga run boundary is not cancellation-checked mid-step | deferred |
| Shutdown while a fence is latched but unconsumed | **Unknown** — no SIGTERM-path fixture | deferred |
| Cloud Run ~10s grace / no renewal loop | ADR context only, not measured | PGN-0024 context (line 49) |

No cancellation/shutdown fixture produced a runtime red on the executed
schedules; the deferred rows stay **Unknown** until executed, and no
delegation claim is made from their absence.

## Scenario matrix

| Scenario | Status |
|---|---|
| matching-owner **success-write** fence at real delivery boundary | TESTED (this increment) |
| matching-owner **failure-state-write** fence at real boundary | TESTED (prior: collect.rs:7815) |
| committed-before-rejected history preservation | TESTED (this increment) |
| rejected-write accounting (fence not counted, run aborts) | TESTED (run aborts `FencedConflict`); slot release wholesale via daemon.rs:3544 test (read-level cite) |
| **production sanctioned rearm helper** (`rearm_after_fenced_conflict` + `resync_event_store`) after real fence | TESTED (this increment, helper level; production wrapper bypassed) |
| committed history after authoritative resync/replay, store-level | TESTED (this increment: `events_with_fibers` — exactly one append per key) |
| idempotent recollection through the rearm sink | TESTED (this increment) |
| cancellation (pre-cancelled token) of rearm run leg after fence → `Cancelled`, history preserved | TESTED (this increment, deterministic, helper-level; mid-backoff/mid-run not covered) |
| in-flight outcomes discarded after fence | TESTED (prior: daemon.rs:3646) |
| stale-owner bypass discard | read-level (daemon.rs:1188); no standalone fixture |
| cancellation mid-resync (blocking resync path) | Unknown — deferred |
| timeout / empty next batch | Unknown — deferred |
| repeated fence / re-fence within a retry | Unknown — deferred |
| shutdown with latched fence | Unknown — deferred |

## Resource inventory (preflight) + gaps

Existing bounds (observed from source, not invented):

| Bound | Value | Source |
|---|---|---|
| collection interval | 3600 s | config/mod.rs:123 |
| collection retry interval | 60 s | config/mod.rs:130 |
| sweep batch deadline | 7200 s (`RuntimeConfig::sweep_timeout`) | config/mod.rs:252 |
| rearm retry | attempts=3, backoff 2 s; **attempt bound, not elapsed** | daemon.rs:473-478 |
| HTTP connect / request / retries | 10 s / 30 s / 2 | config/mod.rs:68-74 |
| GitHub API budget | 4000 calls per window, 3600 s wait | config/mod.rs:175-178 |
| work-queue capacity | 10 000 jobs | config/mod.rs:181 |
| workers | default 16, max 128 | config/mod.rs:47-80 |
| response body / webhook body | 50 MB / 1 MB | config/mod.rs:95, 237 |
| baseline reuse window | 86 400 s | config/mod.rs:145 |

Resource gaps (**Unknown**, not filled with invented constants):

- **Aggregate resync deadline**: no explicit bound beyond per-request HTTP
  timeout; resync spans multiple authoritative reads.
- **Delivery recovery liveness**: no ADR names a producer/consumer of
  `take_run_fence` (ghr-9zl58 GAP); convergence liveness rests on the
  run-boundary code path alone.
- **Shutdown grace**: not measured on a live host.
- **Drain ledger**: no item/byte accounting for work shed by
  `BatchTracker::drain` on a fence.
