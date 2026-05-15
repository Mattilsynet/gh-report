# CHE-0050. Cherry Pit Web CommandRouter Trait

Date: 2026-05-09
Last-reviewed: 2026-05-09
Tier: B
Status: Accepted

## Related

References: CHE-0049:R1, CHE-0014:R2, CHE-0005:R1, CHE-0030, COM-0013:R4

## Context

CHE-0049 R4–R6 and R10 prescribe create/send endpoint behaviour, but the
crate cannot deserialize an arbitrary `C: Command` from request bytes
inside generic code: CHE-0014:R2 forbids `Deserialize` on `Command`.
Without a boundary type, the `/v1/aggregates` POST and
`/v1/aggregates/:id/commands` POST handlers cannot live in
cherry-pit-web. The consumer would either reimplement them in
`extra_routes` (semantic drift from R4/R5/R6/R10) or expose a sum-type
`enum AllCommands: Deserialize + HandleCommand` (pushes back on
CHE-0014:R2 spirit and imposes a sum-type tax on every consumer). A
small `CommandRouter` port owned by the consumer keeps deserialization
and dispatch on the consumer side while leaving status mapping,
correlation echo, idempotency threading, and the `/v1/` surface inside
cherry-pit-web. This ADR ratifies that option.

## Decision

cherry-pit-web exposes a third type parameter `R: CommandRouter` on
`AppState` and `build_router`. The consumer implements the trait,
naming the wire DTO type and translating it to a `CommandGateway` call
on a per-request basis. cherry-pit-web retains all R4/R5/R6/R10
responsibilities; the router is purely the deserialize-and-dispatch hop.

R1 [5]: The crate exposes a `CommandRouter` trait with one associated
type `Wire: serde::de::DeserializeOwned + Send + 'static` and one async
method `dispatch(&self, gateway: &G, ctx: CorrelationContext, idempotency:
Option<IdempotencyKey>, wire: Self::Wire) -> Result<DispatchOutcome,
ErrorEnvelope>` — `Command` itself acquires no `Deserialize` bound,
preserving CHE-0014:R2

R2 [5]: `AppState` and `build_router` gain a third type parameter
`R: CommandRouter<Gateway = G> + Clone + Send + Sync + 'static`
positioned after `G` and `S` — the consumer's `main` instantiates with
concrete `R`, and no `Box<dyn CommandRouter>` or other dynamic dispatch
is permitted, consistent with CHE-0049:R1

R3 [5]: cherry-pit-web retains exclusive ownership of CHE-0049:R4–R6 and
R10 behaviour — status mapping from `ErrorCategory`, correlation
extraction and echo, idempotency-key header threading, and the `/v1/`
DTO contract live in cherry-pit-web handlers; `CommandRouter::dispatch`
returns a typed `Result` and never sets HTTP status, headers, or body
shape

R4 [5]: The trait is object-unsafe by construction (associated type +
generic gateway parameter); cherry-pit-web ships zero blanket impls and
zero default impls, requiring every consumer to provide one explicit
implementation per cherry-pit-web instance per CHE-0005:R1
single-aggregate-per-port

R5 [5]: `CommandRouter` is a deferred-evolution boundary per COM-0013:R4
— if a future ADR adopts an `AllCommands: Deserialize + HandleCommand`
sum-type model or a different deserialization scheme, the trait may be
deprecated by adding a Supersedes ADR; the type-parameter shape allows a
new parameter to coexist with the old during one minor cycle

## Consequences

+ becomes easier: cherry-pit-web ships create + send handlers with
  R4/R5/R6/R10 behaviour intact; no consumer-side reimplementation. The
  `Command` trait stays free of `Deserialize` per CHE-0014:R2.

− becomes harder: every consumer writes one `CommandRouter` impl per
  cherry-pit-web instance — small but non-zero ergonomic tax. The
  `AppState<G, S, R>` ternary type-parameter list is wider than
  CHE-0049's binary form; consumer `main` carries one extra annotation.

risks/migration: if multi-aggregate routing is ever needed in one
cherry-pit-web instance, a future ADR must address it; CHE-0005:R1
currently rules it out so the single-`Wire` shape is safe for v0.1.

## Rejected Alternatives

**Sum-type `enum AllCommands: Deserialize + HandleCommand` on the
consumer side** — pushes back on CHE-0014:R2 (it requires the consumer
to introduce a `Deserialize` impl that wraps Command-bearing variants),
forces every consumer onto a single global sum-type that scales linearly
with command count, and is harder to reverse than a trait — once
consumers have wired the enum into their domain, removing it is a
breaking change. Less reversible per COM-0013:R4.

**Ship only the load handler, leave create/send entirely in
extra_routes** — silently drops CHE-0049:R4/R5/R6/R10 from cherry-pit-web
into consumer code, producing semantic drift between the ADR (which
describes "the web layer's" behaviour) and the crate (which would no
longer host that behaviour). Each consumer reimplements status mapping,
correlation echo, and idempotency threading — exactly the duplication
cherry-pit-web exists to prevent.

**Object-safe `Box<dyn CommandRouter>` shape** — would simplify the
type-parameter list to `AppState<G, S>` carrying a `Box<dyn
CommandRouter>` field, but violates CHE-0049:R1 (no dynamic dispatch
over infrastructure ports) by analogy and breaks CHE-0005:R1 single-
aggregate-per-port binding (object safety would erase the gateway type
parameter the trait must be bound to).

**`Wire` as a generic on `dispatch` rather than an associated type** —
forces the handler call site to name `Wire` at compile time, defeating
the purpose of the abstraction (the handler is generic over `R`; it
cannot know `R::Wire` without an associated type). Rejected for the same
reason `Iterator::Item` is an associated type, not a generic.
