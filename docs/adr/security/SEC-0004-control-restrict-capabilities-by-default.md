# SEC-0004. Control — Restrict Capabilities by Default

Date: 2026-04-28
Last-reviewed: 2026-04-28
Tier: B
Status: Accepted

## Related

References: SEC-0001

## Context

Control concerns the power to influence system behavior. The
primary CISQ threat is elevation of privilege — unauthorized gain
of capabilities enabling unintended actions. In Rust, the type
system and module visibility enforce capability boundaries at
compile time. Unsafe code, ambient authority (global state,
unrestricted filesystem access), and overly broad public APIs
create uncontrolled privilege surfaces.

## Decision

Restrict capabilities by default. Unsafe code is forbidden,
authority is passed explicitly, and privilege surfaces are minimal.

R1 [5]: All crates use `#![forbid(unsafe_code)]` unless an
  explicit safety argument is documented in an ADR
R2 [5]: Authority (file handles, network connections, clocks) is
  passed as explicit parameters, never accessed via globals
R3 [5]: Public API surface is minimal — expose only what
  downstream crates need, nothing more
R4 [7]: Any exception to `forbid(unsafe_code)` requires a
  dedicated ADR with a safety proof sketch

## Consequences

The compiler enforces capability boundaries at zero runtime cost.
Explicit authority passing makes dependency injection natural and
testing straightforward. The constraint eliminates entire
vulnerability classes (buffer overflows, use-after-free) at the
language level. The trade-off is occasional verbosity when threading
authority through call chains.
