# CHE-0078. No Runtime Interceptor Chain

Date: 2026-06-13
Last-reviewed: 2026-06-15
Tier: B
Status: Accepted

## Related

References: CHE-0005, CHE-0050, CHE-0039, CHE-0046, CHE-0024, CHE-0017, CHE-0001

## Context

CommandGateway rustdoc names interceptors as implementor prose
(`crates/cherry-pit-core/src/gateway.rs:18-19` and
`crates/cherry-pit-core/src/gateway.rs:85-88`), but the trait surface is
only `create` and `send`. Solon already threads correlation, retry
categories, dead letters, and policy outputs through typed contracts. The
open question is whether to add a message-level chain.

## Decision

Ratify the absence of a runtime message interceptor chain. P1 correctness
wins over extension convenience: cross-cutting behaviour stays explicit,
typed, and statically wired.

R1 [5]: Do not add a runtime interceptor, middleware, or handler-chain port around command or event dispatch

R2 [5]: Thread correlation through CorrelationContext and EventEnvelope metadata, not ambient handler state

R3 [5]: Express retry, timeout, and cancellation through typed error categories, deadlines, and idempotency keys

R4 [5]: Route failed policy outputs through typed dead-letter records rather than catch-all interceptor hooks

R5 [5]: Keep CommandRouter and policy dispatch object-unsafe by construction, with consumer-owned exhaustive matching

R6 [5]: Treat existing gateway interceptor prose as implementor-local latitude, not a promised substrate API

R7 [5]: Require any future interception proposal to be compile-time typed and ADR-ratified before implementation

R8 [5]: Reject boxed handler chains, dynamic registries, and hidden cross-cutting state in the substrate

## Consequences

+ becomes easier: reviewers can find cross-cutting behaviour in typed
  arguments, envelopes, error categories, and dead-letter records instead of
  searching an ordered handler chain.

− becomes harder: consumers wanting local logging or validation hooks must
  wire them explicitly inside their concrete gateway or adapter code.

risks/migration: no Rust code changes in this Proposed ADR. A later cleanup may
  tighten gateway rustdoc wording, but the absence of a substrate interceptor
  API is now intentional.
