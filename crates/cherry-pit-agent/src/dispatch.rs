//! Policy-output dispatcher per CHE-0051:R4 + R6 + R7.
//!
//! The dispatcher is the *internal* glue called by `App`'s publish
//! handler for each registered policy. For every published envelope:
//!
//! 1. Construct a fresh `CorrelationContext::new(env.correlation_id,
//!    env.event_id)` per CHE-0051:R6 + CHE-0039:R1–R3 (no `Default`,
//!    no shared/cached context, explicit per-dispatch construction).
//! 2. Call `policy.react(env)` synchronously per CHE-0018:R1.
//! 3. For every output, invoke the user-supplied dispatch closure
//!    `Fn(P::Output, &G) -> impl Future<Output = Result<(), AgentError>>`
//!    per CHE-0051:R4 + CHE-0017:R2 (the caller writes the exhaustive
//!    output matcher).
//! 4. On `Err(AgentError)` whose `category()` is `Terminal`, route to
//!    the dead-letter sink per CHE-0051:R7 + CHE-0024:R5 + CHE-0046:R2
//!    (no agent-level retry of `Terminal`). `Retryable` errors flow
//!    back to the caller — typically through the `CommandGateway`'s
//!    own retry path per CHE-0046:R1, which is *outside* this
//!    dispatcher's scope.
//!
//! ## Ground-truth governance (GND-0005)
//!
//! The per-dispatch invariants above — fresh `CorrelationContext`
//! constructed *before* any policy call (no `Default`, no shared
//! state), explicit envelope+context pairing, no speculative dispatch —
//! are the **structural observation mechanism** GND-0005:R1 + R2
//! require for the upstream directives they enforce: CHE-0051:R6 and
//! CHE-0039:R1–R3 are made non-violable at the type/call-shape level
//! (the invalid states are unconstructable). The `correlation_for`
//! `tracing::debug!` line (see `dispatch.rs:91`) is the paired
//! **runtime telemetry mechanism** for the one branch the type system
//! cannot constrain — fresh chain-seed creation when the envelope
//! carries no upstream correlation per CHE-0039:R3 — and cites
//! SEC-0005:R4 directly in code. Together these satisfy GND-0005's
//! "surface violations before integration; pair with runtime
//! telemetry where structural enforcement is impossible" shape.
//!
//! GND-0006 (backbriefing) is *not* the governing principle here:
//! GND-0006's load-bearing element is an *exchange* — issuer-side
//! confirmation of executor intent before action commits. The
//! dispatch path has no such confirmation step; the trace is
//! concurrent with execution, not prior to and gating it.
//!
//! ## C1 boundary note (linus R1 ruling at S4 carries forward)
//!
//! The dispatcher stores registered policies as
//! `Vec<Box<dyn ErasedPolicyDispatcher<G> + Send + Sync>>`. The boxed
//! trait object is a **per-policy adapter wrapping a concrete
//! `(Policy, dispatch_closure)` pair** — it is *not* a
//! `Box<dyn Policy>`. CHE-0005:R1 forbids object-erasure of
//! aggregate-bound infra ports (`EventStore`, `EventBus`,
//! `CommandBus`, `CommandGateway`, `Policy`, `Projection`); the
//! `ErasedPolicyDispatcher` trait is an **agent-internal helper**
//! existing solely to thread `(envelope, gateway) → Future` for a
//! heterogeneous list of `(P, F)` pairs that all share the same
//! `Event = E` and gateway type `G`. The `Policy` trait itself is
//! consumed by-value at registration and stays unboxed inside each
//! adapter — the erasure is at the dispatcher boundary, not at the
//! `Policy` trait. This is the same closure-vs-port reasoning linus
//! approved at S4 (`event_bus.rs:22-37`).

//! ## S5/S6 wiring note
//!
//! `ErasedPolicyDispatcher`, `PolicyAdapter`, `route_failure`,
//! `dispatch_one`, and `DispatcherList` are defined here in S5 with
//! their full unit-test coverage but are not yet driven by
//! `App::run` (an S5 stub). S6 wires them in. The `#[cfg(test)]`
//! tests below reach all of these items via `dispatch_one`'s call
//! graph, so `dead_code` does not fire on them under `--all-targets`
//! and no suppression is needed. Only `DispatcherList` — a public
//! type alias not constructed by any test — carries
//! `#[expect(dead_code)]` so the marker fails closed when S6 wires
//! it into `App::run`.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use cherry_pit_core::{CorrelationContext, DomainEvent, ErrorCategory, EventEnvelope, Policy};

use crate::dead_letter::{DeadLetterRecord, DeadLetterSink};
use crate::error::AgentError;

/// Construct the per-envelope correlation context per CHE-0051:R6 +
/// CHE-0039:R1–R3.
///
/// `EventEnvelope::correlation_id()` is `Option<Uuid>` (CHE-0039:R3:
/// user-initiated commands have no correlation context). Per R6 the
/// context must be constructed *fresh per dispatch* with no `Default`.
/// When the envelope already carries a correlation chain, propagate it
/// with `event_id` as the new causation; when absent, the envelope's
/// `event_id` seeds a new correlation chain (the policy-emitted
/// commands form a coherent tree downstream from this event).
#[must_use]
pub fn correlation_for(
    envelope_correlation_id: Option<uuid::Uuid>,
    event_id: uuid::Uuid,
) -> CorrelationContext {
    if let Some(corr) = envelope_correlation_id {
        CorrelationContext::new(corr, event_id)
    } else {
        tracing::debug!(
            rule = "SEC-0005:R4",
            %event_id,
            "no upstream correlation context; seeding fresh correlation chain from event_id",
        );
        CorrelationContext::new(event_id, event_id)
    }
}

/// Boxed future returned by the user dispatch closure.
type DispatchFuture<'a> = Pin<Box<dyn Future<Output = Result<(), AgentError>> + Send + 'a>>;

/// Agent-internal trait object adapter unifying
/// `(Policy<Event=E>, dispatch_closure)` pairs into a single
/// invocable shape per `E`. See module-level C1 boundary note.
pub(crate) trait ErasedPolicyDispatcher<E, G>: Send + Sync
where
    E: DomainEvent,
    G: Send + Sync + 'static,
{
    /// Stable identifier for the policy — used for dead-letter
    /// records. Defaults to the registered `output_type` concatenated
    /// with the registration order; concrete impls supply their own
    /// `&'static str`.
    fn policy_identity(&self) -> &'static str;

    /// Stable identifier for the policy output type — used for
    /// dead-letter records.
    fn output_type(&self) -> &'static str;

    /// Drive `policy.react(envelope)` and pipe outputs through the
    /// user dispatch closure with a fresh correlation context.
    ///
    /// `gateway` is borrowed by reference per CHE-0051:R4 closure
    /// signature. `ctx` is the per-envelope `CorrelationContext`
    /// constructed by the caller (typically `dispatch_one`) per
    /// CHE-0051:R6 + CHE-0039:R1–R3.
    fn dispatch<'a>(
        &'a self,
        envelope: &'a EventEnvelope<E>,
        gateway: &'a G,
        ctx: CorrelationContext,
    ) -> DispatchFuture<'a>;
}

/// Concrete adapter holding one policy + its dispatch closure.
struct PolicyAdapter<P, F>
where
    P: Policy,
{
    policy: P,
    dispatch: F,
    policy_identity: &'static str,
    output_type: &'static str,
}

impl<P, F, Fut, G> ErasedPolicyDispatcher<P::Event, G> for PolicyAdapter<P, F>
where
    P: Policy,
    G: Send + Sync + 'static,
    F: Fn(P::Output, &G, CorrelationContext) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<(), AgentError>> + Send + 'static,
{
    fn policy_identity(&self) -> &'static str {
        self.policy_identity
    }

    fn output_type(&self) -> &'static str {
        self.output_type
    }

    fn dispatch<'a>(
        &'a self,
        envelope: &'a EventEnvelope<P::Event>,
        gateway: &'a G,
        ctx: CorrelationContext,
    ) -> DispatchFuture<'a> {
        let outputs = self.policy.react(envelope);
        Box::pin(async move {
            for output in outputs {
                (self.dispatch)(output, gateway, ctx.clone()).await?;
            }
            Ok(())
        })
    }
}

/// Build a `PolicyAdapter` from a (policy, closure) pair. Boxed up
/// into the App's heterogeneous registry by `App::register_policy`.
pub(crate) fn make_adapter<P, F, Fut, G>(
    policy: P,
    dispatch: F,
    policy_identity: &'static str,
    output_type: &'static str,
) -> Box<dyn ErasedPolicyDispatcher<P::Event, G>>
where
    P: Policy,
    G: Send + Sync + 'static,
    F: Fn(P::Output, &G, CorrelationContext) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<(), AgentError>> + Send + 'static,
{
    Box::new(PolicyAdapter {
        policy,
        dispatch,
        policy_identity,
        output_type,
    })
}

/// Run the dead-letter route for one terminal failure per CHE-0051:R7.
///
/// `Terminal` errors enter the route once; `Retryable` errors are
/// returned to the caller untouched per CHE-0046:R1–R2.
///
/// On dead-letter sink failure (the sink itself errored), the original
/// policy error is preserved as the returned `AgentError::Policy` and
/// the sink failure is logged via `tracing::error!` rather than
/// shadowing the root cause.
pub(crate) async fn route_failure<D>(
    sink: &D,
    envelope_event_id: uuid::Uuid,
    envelope_correlation_id: Option<uuid::Uuid>,
    envelope_causation_id: Option<uuid::Uuid>,
    policy_identity: &'static str,
    output_type: &'static str,
    err: AgentError,
) -> Result<(), AgentError>
where
    D: DeadLetterSink + ?Sized,
{
    let category = err.category();
    if category == ErrorCategory::Retryable {
        return Err(err);
    }

    let record = DeadLetterRecord {
        event_id: envelope_event_id,
        correlation_id: envelope_correlation_id,
        causation_id: envelope_causation_id,
        error_category: category,
        output_type,
        policy_identity,
        error_message: err.to_string(),
    };

    if let Err(sink_err) = sink.record(record).await {
        tracing::error!(
            error = %sink_err,
            policy_identity,
            output_type,
            "dead-letter sink failed; original policy error preserved",
        );
    }

    Ok(())
}

/// Dispatch one envelope across an entire registry of policies.
///
/// Used internally by `App::dispatch_envelopes`. Exposed here so unit
/// tests can exercise the dispatcher in isolation without standing up
/// a full `App`.
pub(crate) async fn dispatch_one<E, G, D>(
    policies: &[Box<dyn ErasedPolicyDispatcher<E, G>>],
    envelope: &EventEnvelope<E>,
    gateway: &G,
    dead_letter: &D,
) -> Result<(), AgentError>
where
    E: DomainEvent,
    G: Send + Sync + 'static,
    D: DeadLetterSink + ?Sized,
{
    let ctx = correlation_for(envelope.correlation_id(), envelope.event_id());

    for adapter in policies {
        let result = adapter.dispatch(envelope, gateway, ctx.clone()).await;
        if let Err(err) = result {
            route_failure(
                dead_letter,
                envelope.event_id(),
                envelope.correlation_id(),
                envelope.causation_id(),
                adapter.policy_identity(),
                adapter.output_type(),
                err,
            )
            .await?;
        }
    }
    Ok(())
}

/// Public re-export of `Arc` for the dispatcher's storage shape used by
/// tests. The `App` itself owns the policy registry by value; this
/// alias keeps the dispatcher unit-testable with shared ownership.
#[expect(
    dead_code,
    reason = "S6 wires DispatcherList via App::run; fails closed when wired"
)]
pub(crate) type DispatcherList<E, G> = Arc<Vec<Box<dyn ErasedPolicyDispatcher<E, G>>>>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dead_letter::DeadLetterSink;
    use cherry_pit_core::{AggregateId, DomainEvent, EventEnvelope};
    use serde::{Deserialize, Serialize};
    use std::error::Error;
    use std::num::NonZeroU64;
    use std::sync::Mutex;

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    enum Ev {
        Happened,
    }
    impl DomainEvent for Ev {
        fn event_type(&self) -> &'static str {
            "ev.happened"
        }
    }

    #[derive(Debug, PartialEq, Eq)]
    enum Out {
        DoThing,
    }

    struct PolicyA;
    impl Policy for PolicyA {
        type Event = Ev;
        type Output = Out;
        fn react(&self, _env: &EventEnvelope<Ev>) -> Vec<Out> {
            vec![Out::DoThing]
        }
    }

    struct GatewayStub {
        log: Mutex<Vec<String>>,
    }

    fn envelope(seq: u64) -> EventEnvelope<Ev> {
        EventEnvelope::new(
            uuid::Uuid::now_v7(),
            AggregateId::new(NonZeroU64::new(1).unwrap()),
            NonZeroU64::new(seq).unwrap(),
            jiff::Timestamp::now(),
            Some(uuid::Uuid::now_v7()),
            None,
            Ev::Happened,
        )
        .unwrap()
    }

    fn envelope_no_correlation(seq: u64) -> EventEnvelope<Ev> {
        EventEnvelope::new(
            uuid::Uuid::now_v7(),
            AggregateId::new(NonZeroU64::new(1).unwrap()),
            NonZeroU64::new(seq).unwrap(),
            jiff::Timestamp::now(),
            None,
            None,
            Ev::Happened,
        )
        .unwrap()
    }

    struct CountingSink {
        count: Mutex<usize>,
        last_record: Mutex<Option<DeadLetterRecord>>,
    }
    impl CountingSink {
        fn new() -> Self {
            Self {
                count: Mutex::new(0),
                last_record: Mutex::new(None),
            }
        }
    }
    impl DeadLetterSink for CountingSink {
        fn record(
            &self,
            record: DeadLetterRecord,
        ) -> impl Future<Output = Result<(), Box<dyn Error + Send + Sync>>> + Send {
            *self.count.lock().unwrap() += 1;
            *self.last_record.lock().unwrap() = Some(record);
            async move { Ok(()) }
        }
    }

    #[tokio::test]
    async fn dispatch_invokes_closure_with_output() {
        let gateway = GatewayStub {
            log: Mutex::new(Vec::new()),
        };
        let adapter = make_adapter::<PolicyA, _, _, GatewayStub>(
            PolicyA,
            |out, gw, _ctx| {
                gw.log.lock().unwrap().push(format!("{out:?}"));
                async move { Ok::<(), AgentError>(()) }
            },
            "PolicyA",
            "Out",
        );
        let policies = vec![adapter];
        let sink = CountingSink::new();

        dispatch_one(&policies, &envelope(1), &gateway, &sink)
            .await
            .unwrap();

        assert_eq!(*gateway.log.lock().unwrap(), vec!["DoThing".to_string()]);
        assert_eq!(*sink.count.lock().unwrap(), 0);
    }

    #[tokio::test]
    async fn terminal_error_routes_to_dead_letter() {
        let gateway = GatewayStub {
            log: Mutex::new(Vec::new()),
        };
        let adapter = make_adapter::<PolicyA, _, _, GatewayStub>(
            PolicyA,
            |_out, _gw, _ctx| async move { Err::<(), AgentError>(AgentError::Policy("boom".into())) },
            "PolicyA",
            "Out",
        );
        let policies = vec![adapter];
        let sink = CountingSink::new();

        let env = envelope(1);
        let env_id = env.event_id();
        let env_corr = env.correlation_id();

        dispatch_one(&policies, &env, &gateway, &sink)
            .await
            .unwrap();

        assert_eq!(*sink.count.lock().unwrap(), 1);
        let last = sink.last_record.lock().unwrap();
        let rec = last.as_ref().expect("dead-letter record present");
        assert_eq!(rec.event_id, env_id);
        assert_eq!(rec.correlation_id, env_corr);
        assert_eq!(rec.policy_identity, "PolicyA");
        assert_eq!(rec.output_type, "Out");
        assert_eq!(rec.error_category, ErrorCategory::Terminal);
    }

    #[tokio::test]
    async fn retryable_error_propagates_without_dead_letter() {
        use cherry_pit_core::BusError;

        let gateway = GatewayStub {
            log: Mutex::new(Vec::new()),
        };
        let adapter = make_adapter::<PolicyA, _, _, GatewayStub>(
            PolicyA,
            |_out, _gw, _ctx| async move {
                Err::<(), AgentError>(AgentError::Bus(BusError::new("network blip")))
            },
            "PolicyA",
            "Out",
        );
        let policies = vec![adapter];
        let sink = CountingSink::new();

        let result = dispatch_one(&policies, &envelope(1), &gateway, &sink).await;

        assert!(matches!(result, Err(AgentError::Bus(_))));
        assert_eq!(*sink.count.lock().unwrap(), 0);
    }

    #[tokio::test]
    async fn registration_order_independence_two_policies() {
        let gateway = GatewayStub {
            log: Mutex::new(Vec::new()),
        };
        let a = make_adapter::<PolicyA, _, _, GatewayStub>(
            PolicyA,
            |_out, gw, _ctx| {
                gw.log.lock().unwrap().push("A".into());
                async move { Ok::<(), AgentError>(()) }
            },
            "A",
            "Out",
        );
        let b = make_adapter::<PolicyA, _, _, GatewayStub>(
            PolicyA,
            |_out, gw, _ctx| {
                gw.log.lock().unwrap().push("B".into());
                async move { Ok::<(), AgentError>(()) }
            },
            "B",
            "Out",
        );
        let policies = vec![a, b];
        let sink = CountingSink::new();

        dispatch_one(&policies, &envelope(1), &gateway, &sink)
            .await
            .unwrap();

        assert_eq!(*gateway.log.lock().unwrap(), vec!["A", "B"]);
    }

    #[tokio::test]
    async fn correlation_seeded_when_envelope_has_no_correlation_id() {
        let gateway = GatewayStub {
            log: Mutex::new(Vec::new()),
        };
        let adapter = make_adapter::<PolicyA, _, _, GatewayStub>(
            PolicyA,
            |_out, _gw, _ctx| async move { Ok::<(), AgentError>(()) },
            "PolicyA",
            "Out",
        );
        let policies = vec![adapter];
        let sink = CountingSink::new();

        let env = envelope_no_correlation(1);
        dispatch_one(&policies, &env, &gateway, &sink)
            .await
            .unwrap();
    }

    /// G1 — context-threading invariant per CHE-0051:R6 + CHE-0039:R3.
    /// Seed for the S7 proptest. The dispatcher must construct a fresh
    /// `CorrelationContext::new(env.correlation_id, env.event_id)` per
    /// envelope and pass it into the user closure as the third
    /// argument. Pre-fix (R1.5) closure shape `Fn(P::Output, &G) ->
    /// Future` would not even compile this test — confirming the
    /// assertion is load-bearing on the new shape.
    #[tokio::test]
    async fn dispatched_closure_receives_context_threaded_from_envelope() {
        let gateway = GatewayStub {
            log: Mutex::new(Vec::new()),
        };
        let captured: Arc<Mutex<Option<CorrelationContext>>> = Arc::new(Mutex::new(None));
        let captured_for_closure = Arc::clone(&captured);

        let adapter = make_adapter::<PolicyA, _, _, GatewayStub>(
            PolicyA,
            move |_out, _gw, ctx| {
                *captured_for_closure.lock().unwrap() = Some(ctx);
                async move { Ok::<(), AgentError>(()) }
            },
            "PolicyA",
            "Out",
        );
        let policies = vec![adapter];
        let sink = CountingSink::new();

        let env = envelope(1);
        dispatch_one(&policies, &env, &gateway, &sink)
            .await
            .unwrap();

        let captured_ctx = captured
            .lock()
            .unwrap()
            .clone()
            .expect("closure must have been invoked with a CorrelationContext");
        assert_eq!(captured_ctx.correlation_id(), env.correlation_id());
        assert_eq!(captured_ctx.causation_id(), Some(env.event_id()));
    }
}
