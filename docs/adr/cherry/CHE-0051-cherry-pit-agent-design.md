# CHE-0051. Cherry Pit Agent Design

Date: 2026-05-09
Last-reviewed: 2026-05-09

Tier: B
Status: Accepted

## Related

References: CHE-0038, CHE-0005:R1, CHE-0005:R2, CHE-0014:R1, CHE-0014:R2, CHE-0017:R1, CHE-0017:R2, CHE-0018:R1, CHE-0018:R2, CHE-0018:R3, CHE-0020:R3, CHE-0024:R1, CHE-0024:R2, CHE-0024:R3, CHE-0024:R5, CHE-0029:R1, CHE-0029:R5, CHE-0029:R6, CHE-0030:R1, CHE-0001, CHE-0039:R1, CHE-0039:R2, CHE-0039:R3, CHE-0040:R1, CHE-0040:R3, CHE-0041:R1, CHE-0041:R2, CHE-0046:R1, CHE-0046:R2, CHE-0048:R6, CHE-0049:R1, CHE-0050:R4, COM-0011:R2, COM-0011:R4

## Context

cherry-pit-app is the cherry-workspace composition crate — the only one sanctioned to depend on every cherry-pit crate (CHE-0029 dep set: core + gateway + projection + web, lines 50–57) and the only one materialising persist-then-publish, dispatch-then-dead-letter, and replay-from-checkpoint as one composable object. CHE-0005 line 36's open consequence ("wiring complexity scales linearly with aggregate count") is this ADR's answer.

Forces: static-dispatch discipline (CHE-0005:R1, CHE-0049:R1, CHE-0050:R4) keeps every port object-unsafe; persist-then-publish, non-fatal (CHE-0024:R1); per-dispatch correlation (CHE-0039:R1); sync domain / async ports (CHE-0018:R1); deferred orchestration (CHE-0040, CHE-0048:R6, CHE-0024:R3).

Tensions: ergonomics vs static dispatch (CHE-0005:R1), paid as type-parameter expansion (CHE-0005:R1) mirroring CHE-0049 / CHE-0050; conciseness vs runtime neutrality, chosen neutral mirroring CHE-0049's split. CHE-0029's DAG is acyclic; saga pre-decided per CHE-0040.

## Decision

cherry-pit-app ships an `App<G, S, B, P>` composition primitive constructed by explicit struct wiring, an `InProcessEventBus<E>` with a synchronous handler vector, a `register_policy` method taking a per-Policy dispatch closure, a heterogeneous tuple of `ProjectionDriver` instances driven by an extension trait local to this crate, a `DeadLetterSink` trait with a `TracingDeadLetterSink` default, and an `App::run(shutdown)` future that the consumer's binary drives under its own `#[tokio::main]`. Multi-aggregate composition expands the type-parameter list rather than hiding behind dynamic dispatch. Per-policy durable checkpoints, durable dead-letter sinks, runtime ownership, saga orchestration, and WebSocket subscription are all explicitly out of scope for v0.1.

R1 [5]: cherry-pit-app depends on `cherry-pit-core`, `cherry-pit-gateway`, `cherry-pit-projection`, and `cherry-pit-web` (per CHE-0029 line 50–57 acyclic DAG, web upstream of agent), uses `tokio` and `tracing` as runtime adapter deps (CHE-0029:R5), carries `#![forbid(unsafe_code)]`, and exposes a flat public API via private modules with `pub use` re-exports per CHE-0030:R1 — `App`, `InProcessEventBus`, `DeadLetterSink`, `DeadLetterRecord`, `TracingDeadLetterSink`, `ProjectionDriverExt`, `AgentError` live at the crate root

R2 [5]: `InProcessEventBus<E>` stores handlers as a `Vec<HandlerFn>` registered through an impl-specific method and invoked synchronously inside `publish`, satisfying CHE-0024:§7 in-process delivery semantics — the `cherry-pit-core::EventBus` port itself acquires no `subscribe` method (CHE-0024:R2), and one `InProcessEventBus<E>` instance exists per aggregate type per CHE-0005:R1, with multi-aggregate composition handled by parameter expansion (R9) rather than a heterogeneous handler registry

R3 [5]: composition uses explicit struct construction `App::new(gateway, store, bus, projections)` mirroring CHE-0049's `AppState::new` minimalism — no type-state builder, no fluent builder returning `Result<App, WiringError>`, no constructor macro; the `App<G, S, B, P>` type-parameter list expresses the wired graph shape directly and wiring completeness is enforced at compile time by parameter inference rather than by accumulator state

R4 [5]: `App::register_policy<P>(policy: P, dispatch: F)` takes a per-Policy closure `F: Fn(P::Output, &G, CorrelationContext) -> impl Future<Output = Result<(), AgentError>> + Send` — caller writes the exhaustive output matcher per CHE-0017:R2; the dispatcher constructs `CorrelationContext::new(envelope.correlation_id, envelope.event_id)` per envelope (R6) and passes it as the third argument so the closure threads it into `gateway.send(...)`; static dispatch end-to-end (no `Box<dyn Policy>`, no `Box<dyn Fn>`)

R5 [5]: multi-projection composition is a heterogeneous tuple `(D1, D2, …, Dn)` of `ProjectionDriver` instances with fixed arity ~8, parameterised on the App's storage adapter, satisfying CHE-0048:R6's deferral by giving compile-time wiring completeness via type inference; the missing `apply_one` per-event primitive is added by `ProjectionDriverExt`, declared inside cherry-pit-app and importing `ProjectionDriver` from cherry-pit-projection without editing it (CHE-0048 stays untouched per FOCUS.md §8)

R6 [5]: the policy-output dispatcher constructs a fresh `CorrelationContext::new(envelope.correlation_id, envelope.event_id)` for every dispatch invocation, threading it through the user-provided closure — there is no `Default` impl on `CorrelationContext` per CHE-0039:R2, no shared/cached context across dispatches, and no implicit task-local propagation; the contract is explicit per-dispatch construction per CHE-0039:R1 and R3

R7 [5]: failed policy outputs route to a `DeadLetterSink` trait whose `record` method receives a `DeadLetterRecord` carrying `event_id`, `correlation_id`, `causation_id`, `error_category`, `output_type`, and `policy_identity` (verbatim from CHE-0024:R5 and CHE-0040:R3); `TracingDeadLetterSink` is the v0.1 default emitting `tracing::error!`; `Retryable` errors flow through the `CommandGateway` retry path per CHE-0046:R1 without dead-lettering, `Terminal` errors enter once with no agent-level retry (CHE-0046:R2)

R8 [5]: `App::run(shutdown: impl Future<Output = ()>) -> impl Future<Output = Result<(), AgentError>> + Send` is the lifecycle entrypoint — the consumer's binary owns `#[tokio::main]` and signal handling, calls `app.run(tokio::signal::ctrl_c()).await`, and the agent does not configure the runtime, mirroring CHE-0049's `build_router`-returns-`Router` shape where the consumer drives `axum::serve`

R9 [5]: multi-aggregate composition is type-parameter expansion per CHE-0005:R2 — each aggregate adds its `(G_n, S_n, B_n, P_n)` quartet to `main`, producing one typed `App` per aggregate or a wrapping struct owning N `App` instances; aggregate identity is never erased into a heterogeneous registry; wiring-LOC is the price of static dispatch (FOCUS.md §4 step 5 verifies wiring does not dominate domain code)

R10 [5]: per-policy durable checkpoints are out of scope for v0.1 — in-process synchronous publish (CHE-0024:§7) plus `Policy::react` purity (CHE-0041:R2) plus aggregate-level idempotency (CHE-0041:R1) jointly mean restart-time replay-from-store with the projection's checkpoint as the watermark is sufficient for at-least-once policy execution; the gap is documented and a future ADR may introduce policy checkpoints when an at-least-once consumer crosses an out-of-process boundary

R11 [5]: persist-then-publish is enforced by App composition — `EventStore::append` returns before `EventBus::publish` fires (CHE-0024:R1); publish failure is non-fatal and routes to dead-letter, not a rollback; `Policy::react` and `Projection::apply` execute synchronously inside the publish path (CHE-0018:R1–R2) with async confined to the port boundary; the agent never invents `AggregateId` (CHE-0020:R3); `Command` carries no `Clone`/`Debug`/`Serialize` bounds (CHE-0014:R1–R2)

## Consequences

Given up, per COM-0011:R4:

- **Durable dead-letter sinks**: deferred — v0.1 ships `TracingDeadLetterSink`; consumers implement `DeadLetterSink`.
- **Per-policy durable checkpoints**: deferred — replay-from-store suffices under CHE-0024 in-process semantics; needed once a policy crosses an out-of-process boundary.
- **`App::run` owning tokio**: deferred — consumer's binary owns `#[tokio::main]`; deliberate runtime-neutrality cost.
- **Saga orchestrator**: permanently deferred per CHE-0040.
- **WebSocket subscription**: deferred per CHE-0049:R3.

Each aggregate expands the consumer's type list; the projection tuple's ~8-arity ceiling requires macros or a builder revisit if crossed (FOCUS.md §4 step 5 measures wiring-LOC vs domain-LOC). Wiring completeness is local — the agent cannot guarantee the consumer registered every Policy. Cross-context subscription per CHE-0005:R3 uses upstream policies emitting commands to downstream gateways, not a shared bus.

## Rejected Alternatives

**Type-state builder ensuring compile-time wiring completeness across all registered Aggregates** (Q2 alternate (a)) — would express the full wired graph as an accumulating type, guaranteeing at compile time that every registered Aggregate has a Store, Bus, Gateway, ≥0 Policies, and ≥0 Projections wired before `.build()`. CHE-0048:R6 floats this. Rejected because the type-parameter list explodes with each aggregate added (each builder method produces a new type), generic error messages become unreadable, and the compile-time guarantee duplicates what `App::new`'s explicit type-parameter list already enforces by inference. CHE-0049's `AppState::new` precedent is the simpler shape, and CHE-0001's simplicity ranking above flexibility tips the scale.

**`tokio::sync::broadcast` as the EventBus implementation** (Q1 alternate (b)) — would provide built-in fanout to N async subscribers with bounded-channel lag handling. Rejected because CHE-0024:§7 specifies in-process delivery is "synchronous within publish," which the synchronous handler vector satisfies directly; a broadcast channel introduces lag-handling and dead-letter-on-lag complexity that has no v0.1 consumer need and adds a second async surface inside what the rule says is a synchronous step. Pluggable trait `AgentEventBus` distinct from `cherry-pit-core::EventBus` (Q1 alternate (c)) is also rejected: it duplicates the port surface for no concrete v0.1 second impl.

**Trait `DispatchOutput<O>` blanket-implemented per `(Policy, Gateway)` pair** (Q3 alternate (b)) — would require the consumer to derive or impl `DispatchOutput` for each output enum, with the agent calling the trait method per output. Rejected because the closure shape is shorter at the call site, the trait shape requires the consumer to introduce a named impl block per Policy with no expressivity gain, and macro generation of dispatch glue (Q3 alternate (c)) defers a design decision behind a macro surface that is itself a maintenance liability — both alternates trade closure-call ergonomics for ceremony with no compensating compile-time guarantee since CHE-0017:R2's exhaustive-match obligation is satisfied identically by the closure body.

**Type-state builder accumulating a heterogeneous projection list (HList-style)** (Q4 alternate (b)) — would let the projection collection grow without a fixed-arity ceiling. Rejected because the resulting types are unergonomic to name (consumers cannot easily write the App's full type), error messages become opaque, and the practical upper bound of ~8 projections per aggregate covers every v0.1 consumer. One projection driver task per projection with no in-agent composition (Q4 alternate (c)) is also rejected: it loses CHE-0048:R6's "compile-time wiring completeness" intent and pushes the wiring to the consumer's spawn site.

**File-per-failed-output `DeadLetterSink` impl as the v0.1 default** (Q5 alternate (a)) — would mirror CHE-0048's atomic temp-file-then-rename pattern, providing durable operator-inspectable records out of the box. Rejected because the durability decision belongs to the consumer's operational story (CHE-0047 runbook scope), the file format and rotation policy would require its own ADR to bind, and shipping a tracing-only default lets the trait stabilise before the durable impl is designed. Bounded in-memory channel exposed via accessor (Q5 alternate (b)) is also rejected: ephemeral by definition and adds an accessor surface that is harder to deprecate than a trait impl.

**Agent owns `#[tokio::main]` via `App::run_to_completion()`** (Q7 alternate (a)) — would make the agent a turn-key application surface. Rejected because the consumer's binary needs to compose the agent with HTTP (`cherry-pit-web::build_router` returns a `Router` the consumer drives) and signal handling that may need to coordinate across both surfaces; runtime ownership in the agent forces the consumer to rebuild the runtime topology if any second async surface is added. The shipping-both variant (`App::spawn` + `App::run_to_completion`, Q7 alternate (c)) is also rejected: it doubles the lifecycle surface for no concrete v0.1 consumer need and creates two ways to do the same thing.

<!--
Q-choice → R-rule mapping (plan-only commentary, not normative):
Q1 (in-process synchronous handler vector)  → R2
Q2 (explicit struct construction)            → R3
Q3 (per-Policy dispatch closure)             → R4  [Q3 amended 2026-05-09 per FOCUS.md §7: closure shape now `Fn(P::Output, &G, CorrelationContext) -> Future` so the dispatcher mechanically threads the per-envelope context to the closure per R6]
Q4 (heterogeneous tuple of ProjectionDriver) → R5
Q5 (DeadLetterSink trait + tracing default)  → R7
Q6 (no durable per-policy checkpoints v0.1)  → R10
Q7 (App::run owned by consumer's binary)     → R8
Cross-cutting: C4 (CorrelationContext per dispatch) → R6;
              C7 (pub use re-export discipline)     → R1;
              C12 (multi-aggregate parameter expansion) → R9;
              C2/C8/C16 (persist-then-publish, sync domain, no unsafe) → R11.
-->
