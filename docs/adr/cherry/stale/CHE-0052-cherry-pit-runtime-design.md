# CHE-0052. Cherry Pit Runtime Design

Date: 2026-05-09
Last-reviewed: 2026-05-09

Tier: B
Status: Superseded by CHE-0055

## Retirement

Superseded-by: CHE-0055
Moved-to-stale: 2026-05-13
Reason: CHE-0055 (`cherry-pit-wq`) ships the narrower work-queue/worker-pool
surface this ADR proposed under a more accurate crate name, with the
correlation propagation reach (R4/R6) explicitly deferred to v0.2 per
FOCUS.md §7+§8 ratification (surprise bead `adr-fmt-tm6m`). The full
five-module donor absorption envisioned here (budget, rate-limit, pagination
helpers in addition to work queue / worker pool) is collapsed under CHE-0055's
verbatim-port scope; the rejected-alternatives reasoning below is retained
for historical record.

## Related

References: CHE-0038, CHE-0005:R1, CHE-0007:R1, CHE-0007:R2, CHE-0007:R3, CHE-0018:R1, CHE-0018:R2, CHE-0018:R3, CHE-0022:R1, CHE-0029:R1, CHE-0029:R2, CHE-0029:R5, CHE-0029:R6, CHE-0030:R1, CHE-0030:R2, CHE-0001, CHE-0039:R1, CHE-0039:R2, CHE-0039:R3, CHE-0046:R1, CHE-0046:R2, CHE-0046:R3, CHE-0046:R5, CHE-0051:R1, CHE-0051:R6, CHE-0051:R8

## Context

cherry-pit-runtime is a new workspace crate absorbing the domain-agnostic concurrency and pacing primitives currently under `crates/quics-aggregate/`: bounded deduplicated FIFO `WorkQueue<C>`, `JobExecutor`-driven `WorkerPool`, CAS-based `BudgetGate`, atomic `RateLimitState` reading `X-RateLimit-*` headers, and `Link`-header pagination helpers. Two donor modules (`aggregate_store`, `event_bus`) are already absorbed elsewhere (CHE-0051:R2). Per CHE-0029:R1/R5 cherry-pit-runtime sits as a new tokio-using adapter peer of `cherry-pit-gateway`/`cherry-pit-projection`/`cherry-pit-web`; no other cherry-pit crate may depend on it in v0.1 (CHE-0051:R1 fixes agent's deps). CHE-0018:R3 holds: cherry-pit-runtime depends on `cherry-pit-core`, not the other way around. BC-7 (CHE-0039 — R1/R2/R3) is the central tension — option (b) selected: carry `CorrelationContext` on `JobSpec<C>`, mirroring CHE-0042-style envelope correlation. Runtime ownership stays in the consumer binary (CHE-0049, CHE-0051:R8). Sagas (CHE-0040), distributed/durable queues, metrics façades, and replacement of CHE-0051:R2's dispatch loop are explicitly out of scope.

## Decision

cherry-pit-runtime ships `WorkQueue<C>`, `JobSpec<C>`, `JobSource`, `EnqueueResult`, `BatchEnqueueResult`, `BatchTracker`, `enqueue_batch`, `JobExecutor`, `WorkerPoolConfig`, `JobOutcome<R>`, `run_worker_pool`, `shutdown_worker_pool`, `BudgetGate`, `RateLimitState`, `HALT_THRESHOLD`, `WARN_THRESHOLD`, `next_url`, and `next_url_same_origin` as a `pub use`-flat surface (CHE-0030:R1) over private modules. The crate depends on `cherry-pit-core` (for `CorrelationContext` per BC-7), `tokio`, `tracing`, `http`, `scc`, and `futures-util`; it MUST NOT be cited as a dependency by any other cherry-pit crate in v0.1. `JobSpec<C>` gains a `CorrelationContext` field (option (b) of the BC-7 alternatives) so that the existing `JobExecutor::execute` shape stays minimal and consumers extract correlation inside their impl. The crate is runtime-neutral — the consumer's binary owns `#[tokio::main]`, mirroring CHE-0051:R8 — and graceful shutdown follows the donor's `shutdown_worker_pool(handles, timeout)` shape, reconciled with CHE-0046:R5 (cancellation does not imply rollback). Saga orchestration, distributed queues, durable job state, metrics façades, and any replacement of `cherry-pit-agent`'s dispatch loop are explicitly deferred to v0.2 or to other crates.

R1 [5]: cherry-pit-runtime's `[dependencies]` MUST contain only `cherry-pit-core`, `tokio`, `tracing`, `http`, `scc`, and `futures-util` — no other cherry-pit crate. It MUST NOT be added as a dependency of `cherry-pit-{core, gateway, projection, web, agent}` in v0.1 (would break CHE-0051:R1 or fail CHE-0029:R6's `cargo tree` check). Only application-tier consumers (gh-report and future drivers) may depend on it.

R2 [5]: cherry-pit-runtime carries `#![forbid(unsafe_code)]` at the crate root per CHE-0007:R1 and CHE-0007:R3, and contains no `unsafe` blocks, `unsafe impl`, or `unsafe fn` bodies per CHE-0007:R2 — the donor `quics-aggregate` already conforms and the surgical extract preserves the property; BC-14 satisfied by construction.

R3 [5]: cherry-pit-runtime exposes its public API via private modules with selective `pub use` re-exports per CHE-0030:R1, with the flat surface enumerated in Decision above; internal modules (`work_queue`, `worker_pool`, `budget`, `rate_limit`, `pagination`) are implementation detail per CHE-0030:R2 and may be reorganised without a SemVer-major bump.

R4 [5]: `JobSpec<C>` carries a `pub correlation: CorrelationContext` field (from `cherry-pit-core`); `JobSpec::new` and `enqueue_batch` accept a `CorrelationContext` argument explicitly per CHE-0039:R2 (no `Default`). `JobExecutor::execute` retains `(&DomainKey, &Self::Context)`; impls extract correlation by reading `job.correlation`, building downstream contexts via CHE-0039:R3 when calling `CommandGateway::send` per CHE-0039 propagation flow [R4 amended 2026-05-11: v0.1 conformance deferred to v0.2 — see Open/Deferred; surprise bead adr-fmt-tm6m].

R5 [5]: cherry-pit-runtime is runtime-neutral — no constructor calls `tokio::runtime::Runtime::new()` or `Builder::*`. `run_worker_pool` and `WorkQueue::dequeue` assume an active tokio context; the consumer binary owns `#[tokio::main]` and signal handling, mirroring CHE-0049 and CHE-0051:R8. CHE-0018:R3 satisfied: cherry-pit-core gains no transitive tokio dep.

R6 [5]: `JobOutcome<R>` MUST carry the `CorrelationContext` from the originating `JobSpec<C>` on both `Success` and `Failure` variants, so cancellation/error paths preserve BC-7 — dead-letter sinks observe the same correlation chain per CHE-0046:R6 [R6 amended 2026-05-11: v0.1 conformance deferred to v0.2 — see Open/Deferred; surprise bead adr-fmt-tm6m].

R7 [4]: graceful shutdown follows the donor `shutdown_worker_pool(handles, timeout)` contract — workers complete current `JobExecutor::execute` or are aborted at timeout — reconciled with CHE-0046:R5: cancellation does NOT imply rollback of side effects already committed by the executor body. Recovery semantics are the consumer's responsibility.

R8 [4]: cherry-pit-runtime is in-process only — `WorkQueue` state lives in tokio mpsc channels plus `scc::HashSet` dedup; process restart drops the queue. Consumers needing durability layer their own `EventStore`-backed outbox above per CHE-0024's persist-then-publish ordering. Distributed/persistent variants deferred to v0.2.

R9 [4]: cherry-pit-runtime emits observability via `tracing` only — no Prometheus exporter, no OpenTelemetry façade, no metrics-trait abstraction in v0.1; `WorkerPoolConfig` does not carry a metrics field; consumers needing structured metrics subscribe to the existing `tracing` events (`info!`, `warn!`, `debug!` preserved verbatim from the donor) and bridge with their own subscriber.

R10 [4]: tests in cherry-pit-runtime follow CHE-0038 — units alongside modules, integration under `crates/cherry-pit-runtime/tests/`, `tempfile` plus real tokio runtimes over mock frameworks per CHE-0038:R5. Donor unit tests (FIFO ordering, dedup, capacity, channel close, batch tracker) are absorbed verbatim plus a BC-7 propagation property test on cancellation paths.

R11 [4]: the cherry-pit-runtime public surface is additive-only across SemVer-minor bumps per CHE-0022:R1 — adding types or re-exports is minor; renaming/removing any R3 item is major. `#[non_exhaustive]` markers preserved from the donor on `JobSpec`, `JobSource`, `EnqueueResult`, `JobOutcome`, `WorkerPoolConfig`, `BatchEnqueueResult`, `BatchTracker`, `RateLimitState`, `BudgetGate`.

## Consequences

**Positive.** gh-report and future cherry-pit consumers gain a stable worker-pool harness independent of the dismantled `quics-*` lineage. BC-7 correlation reaches cancellation/error paths previously blind in the donor, via `JobSpec`/`JobOutcome` carrying `CorrelationContext` (CHE-0046:R6). The crate sits cleanly on the async side of CHE-0018; `cherry-pit-core`'s transitive closure is unchanged. Runtime-neutrality (R5) matches the precedent of CHE-0049 and CHE-0051:R8.

**Negative.** `JobSpec<C>` gains a required `CorrelationContext` field — breaking vs the donor's `JobSpec::new(domain_key, context, source)`. Because `CorrelationContext` has no `Default` (CHE-0039:R2), every call site must explicitly pick `none()`, `correlated(id)`, or `new(corr, cause)`. This is the deliberate price of CHE-0039 reaching the worker-pool boundary; the cost surfaces correlation forgetting at compile time. `JobOutcome<R>` variants enlarge similarly.

**Open / deferred.** Distributed/persistent queues (R8) and a metrics façade beyond `tracing` (R9) defer to v0.2. R4/R6's correlation propagation also defers to v0.2 per FOCUS.md §7+§8 (user-ratified 2026-05-11): the M4.A7 verbatim port surfaced that donor `quics_aggregate::JobSpec::new(domain_key, context, source)` has no `CorrelationContext` and no v0.1 consumer needs CHE-0039 reach through the worker-pool boundary (oracle audit `adr-fmt-ecpv`, surprise `adr-fmt-tm6m`). Until reinstated, `JobSpec`/`JobOutcome` mirror the donor verbatim; R4/R6 [5] markers cover v0.2+ scope only. Sagas remain permanently out per CHE-0040. Unabsorbed donor modules (`aggregate_store`, `event_bus`) stay superseded by `cherry-pit-gateway::MsgpackFileStore` + `cherry-pit-projection` and CHE-0051:R2 respectively.

## Rejected Alternatives

**Inline-absorb the runtime primitives into gh-report (D1.a of the WU-6 plan-mode decision matrix).** Would have collapsed all five donor modules into `crates/gh-report/src/infra/` with no new cherry-pit crate. Rejected at plan-mode ratification (D1.b chosen): the runtime primitives are domain-agnostic by design (the donor lib.rs explicitly says "domain-agnostic building blocks for data collection pipelines"), and burying them inside an application crate forfeits any future cherry-pit consumer's ability to reuse them. The WU-6 brief at line 31 records this decision; cherry-pit-runtime is the codified outcome.

**Single combined `cherry-pit-infra` crate covering both runtime primitives and storage primitives** (the parallel A3 sub-mission). Rejected because the two clusters have orthogonal dependency profiles: cherry-pit-runtime requires tokio (CHE-0029:R5 adapter posture); cherry-pit-storage-primitives is pure I/O on `std::fs` and does not need tokio. Combining them would force every consumer of the storage primitives to take a tokio dependency, inflating the BC-13 transitive closure of any storage-primitive consumer for no benefit and violating the spirit of CHE-0029:R4's leaf-discipline reasoning (which restricts cherry-pit-core but applies the same dependency-minimisation principle to peer adapter crates).

**Keep donor `quics-aggregate` symbols in place under a re-export shim.** Would have left `crates/quics-aggregate/` in the workspace and added a thin `cherry-pit-runtime` crate that `pub use`d its symbols. Rejected because the WU-6 brief's Phase B step B9 explicitly removes `quics-aggregate` from `[workspace.members]`, the cherry-pit-naming + DAG-hygiene commitment requires the symbols to live under a cherry-pit-named crate (not a shim), and the shim would create two public surfaces for the same types — a maintenance liability with no compensating benefit. Surgical extract per A7 is the chosen path.

**Add `CorrelationContext` as a parameter on `JobExecutor::execute` (option (a) of the BC-7 alternatives).** Would have changed the trait method signature to `execute(&self, &DomainKey, &Self::Context, CorrelationContext) -> Future<…>`. Rejected in favour of option (b) for three reasons: (i) every `JobExecutor` impl regardless of correlation needs would have to thread a parameter it may never use, paying a per-impl ceremony tax; (ii) `enqueue_batch` would have to thread correlation per-job through the queue rather than stamp it once at the batch level, losing the natural batch-as-correlation-unit affordance; (iii) carrying correlation as a public field of `JobSpec` mirrors how `EventEnvelope` carries `correlation_id` on the storage side (CHE-0042 lineage), so consumers see one consistent pattern across both the work-unit envelope and the event envelope. Option (a) remains a reasonable design and could be added later as a parallel convenience without breaking option (b)'s consumers; option (b) is the load-bearing surface.

**cherry-pit-runtime owns `#[tokio::main]` via a `RuntimeHarness::run_to_completion()` entrypoint.** Would have made the crate a turn-key worker-pool application surface, mirroring the rejected CHE-0051 Q7 alternate (a). Rejected for the same reason: the consumer's binary needs to compose the worker pool with HTTP (`cherry-pit-web::build_router`), with `cherry-pit-agent::App::run`, and with signal handling that may need to coordinate across all three async surfaces. Runtime ownership in cherry-pit-runtime would force the consumer to rebuild the runtime topology if any second async surface is added, exactly the cost CHE-0051:R8 already declined to pay.
