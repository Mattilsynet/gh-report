# cherry-pit-app

Root composition crate wiring `Aggregate` / `Policy` / `Projection` against
`EventStore` / `EventBus` / `CommandGateway`.

This is the only crate in the cherry-pit family sanctioned to depend on every
other cherry-pit crate (per [CHE-0029] acyclic-DAG rule). Public API is exposed
flat at the crate root via `pub use` re-exports per [CHE-0030]:R1 — internal
module structure is implementation detail.

## Public API

- **`App<G, S, B, P, D>`** — the composition struct. Five type parameters cover
  the sanctioned surface without `Box<dyn>` over any infra port ([CHE-0005]:R1).
- **`App::new(gateway, store, bus, projections, dead_letter)`** — explicit
  constructor; no `Default` per [CHE-0039]:R2 + [CHE-0051]:R3.
- **`App::register_policy(policy, dispatch_closure, identity, output_type)`** —
  per-policy registration; the closure is the caller-written exhaustive output
  matcher per [CHE-0017]:R2.
- **`App::run(shutdown)`** / **`App::run_until_ctrl_c()`** — drive the publish
  loop. Requires a multi-thread tokio runtime.
- **`InProcessEventBus<E>`** — synchronous fan-out `EventBus` impl per
  [CHE-0024]:§7 + [CHE-0051]:R2.
- **`DeadLetterSink`** — trait for terminal-failure routing per [CHE-0051]:R7 +
  [CHE-0024]:R5 + [CHE-0040]:R3. Default impl: **`TracingDeadLetterSink`**.
- **`DeadLetterRecord`** — diagnostic payload (`event_id`, `correlation_id`,
  `causation_id`, `error_category`, `output_type`, `policy_identity`,
  `error_message`).
- **`AgentError`** — `#[non_exhaustive]` error type. Variants: `Policy`,
  `DeadLetter`, `Store`, `Bus`. `category()` returns retry guidance per
  [CHE-0046]:R1–R2.
- **`ProjectionDriverExt`** + **`ProjectionDriverTuple`** — per-event
  projection primitive + heterogeneous tuple shape (arities 0..=2) per
  [CHE-0051]:R5.
- **`CorrelationContext`** — re-exported from `cherry-pit-core` per
  [CHE-0030]:R1; threaded through every dispatch closure per [CHE-0051]:R6.
- **`correlation_for(envelope_correlation_id, event_id)`** — helper that
  constructs the per-envelope context per [CHE-0051]:R6 + [CHE-0039]:R3.

## Minimal composition example

```rust,no_run
use cherry_pit_app::{
    AgentError, App, CorrelationContext, InProcessEventBus, TracingDeadLetterSink,
};

# // Stand-in types so the example shape is self-contained.
# struct MyGateway;
# struct MyStore;
# struct MyPolicy;
# // (in real code these implement CommandGateway / EventStore / Policy)
# impl MyGateway { fn new() -> Self { Self } }
# impl MyStore { fn new() -> Self { Self } }
# impl MyPolicy { fn new() -> Self { Self } }

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let gateway = MyGateway::new();
    let store = MyStore::new();
    let bus = InProcessEventBus::new();
    let sink = TracingDeadLetterSink::new();

    let mut app = App::new(gateway, store, bus, /* projections */ (), sink);

    app.register_policy(
        MyPolicy::new(),
        // Caller-owned exhaustive output matcher (CHE-0017:R2).
        // `ctx` is the per-envelope CorrelationContext (CHE-0051:R6) —
        // thread it into any gateway.send(...) so policy-emitted
        // commands inherit the correlation chain.
        |_output, _gateway, _ctx: CorrelationContext| async move {
            Ok::<(), AgentError>(())
        },
        "MyPolicy",
        "MyPolicyOutput",
    );

    app.run_until_ctrl_c().await?;
    Ok(())
}
```

## Dead-letter contract

When a registered policy's dispatch closure returns `Err(AgentError)` whose
`category()` is `Terminal`, the agent constructs a `DeadLetterRecord` and
calls `DeadLetterSink::record(...)`. The publish loop **does not abort** —
one bad envelope must not stop the bus ([CHE-0051]:R7 + [CHE-0046]:R2).
`Retryable` errors are surfaced via `tracing::error!` for caller-side retry
orchestration; they do **not** enter the dead-letter route ([CHE-0046]:R1).

The default `TracingDeadLetterSink` emits one structured `tracing::error!`
per record. Consumers needing durable persistence implement `DeadLetterSink`
against their preferred backend.

## Type discipline

- One `App` per aggregate ([CHE-0051]:R9 + C12). Multi-aggregate composition
  expands the type-parameter list at the consumer site, never via a
  heterogeneous registry.
- No `Box<dyn>` over the six aggregate-bound infra ports ([CHE-0005]:R1).
  The dispatcher's internal `Box<dyn ErasedPolicyDispatcher>` is over
  user closures, not over the `Policy` trait — see the C1 boundary notes
  in `dispatch.rs` and `event_bus.rs`.
- `App::run` requires a multi-thread tokio runtime — the synchronous bus
  callback bridges to async via `Handle::block_on`, which deadlocks on the
  single-thread flavour.

## ADR pointers

- [CHE-0005] — single-aggregate design; no `Box<dyn>` over infra ports.
- [CHE-0017] — policy outputs are statically typed; caller writes the matcher.
- [CHE-0024] — event-delivery model; in-process delivery synchronous within
  `publish`; dead-letter field set.
- [CHE-0030] — flat public API via crate-root `pub use` re-exports.
- [CHE-0038] — testing strategy (relevant to consumers wiring tests against
  this surface).
- [CHE-0039] — correlation-context propagation; no `Default`.
- [CHE-0040] — saga / compensation pattern; dead-letter field set.
- [CHE-0046] — retry / timeout / cancellation; `Retryable` vs `Terminal`.
- [CHE-0048] — `cherry-pit-projection` design (extension trait lives here per
  [CHE-0051]:R5).
- **[CHE-0051]** — this crate's design ADR. Read first for binding rules.

[CHE-0005]: ../../docs/adr/cherry/CHE-0005-single-aggregate-design.md
[CHE-0017]: ../../docs/adr/cherry/CHE-0017-policy-output-static-type.md
[CHE-0024]: ../../docs/adr/cherry/CHE-0024-event-delivery-model.md
[CHE-0029]: ../../docs/adr/cherry/CHE-0029-crate-decomposition.md
[CHE-0030]: ../../docs/adr/cherry/CHE-0030-flat-public-api.md
[CHE-0038]: ../../docs/adr/cherry/CHE-0038-testing-strategy.md
[CHE-0039]: ../../docs/adr/cherry/CHE-0039-correlation-context-propagation.md
[CHE-0040]: ../../docs/adr/cherry/CHE-0040-saga-compensation-pattern.md
[CHE-0046]: ../../docs/adr/cherry/CHE-0046-retry-timeout-cancellation-semantics.md
[CHE-0048]: ../../docs/adr/cherry/CHE-0048-cherry-pit-projection-design.md
[CHE-0051]: ../../docs/adr/cherry/CHE-0051-cherry-pit-agent-design.md

## Operational caveats (v0.1)

Two known limitations in this v0.1 closure that consumers should plan
around:

- **Bus-producer absence.** The agent registers a synchronous fan-out
  *handler* against the bus inside `App::run` (per [CHE-0024]:R2 +
  [CHE-0051]:R2), but no `CommandBus`-driven publish path is wired
  end-to-end in v0.1: the `App` owns the bus by value and consumes it
  when `run` registers the dispatcher, so there is no public handle
  through which a test or consumer can publish envelopes through the
  same `App` instance. The publish loop is exercised at the unit
  level (`dispatch.rs` tests cover the dispatcher in isolation); the
  end-to-end publish→dispatch→dead-letter integration test is
  deferred per [CHE-0024]:R1 and tracked at bd `adr-fmt-1art` (S8+).
  Consumers that need to drive publication today must wrap the bus
  before handing it to `App::new` and publish through their own clone.
- **Projection-tuple arity ceiling 0..=2.** `ProjectionDriverTuple`
  is implemented for tuples of arity 0, 1, and 2 only (per
  [CHE-0051]:R5; rationale GND-0004 — *deviation permitted and
  reported* — was reviewed POSITIVE for v0.1). Wiring three or more
  projections against one `App` requires either widening
  `ProjectionDriverTuple` (post-v0.1 ADR amendment work) or composing
  multiple sub-drivers behind a single tuple slot. Hitting the
  ceiling is a signal to inspect `ProjectionDriverTuple` directly
  before reaching for a workaround.

## License

Dual licensed under Apache-2.0 OR MIT.
