# SEC-0013. WASM Gate G2: Domain-App Plugin Capability and Trust Model

Date: 2026-08-01
Last-reviewed: 2026-08-01

Tier: B
Status: Accepted
Crates: gh-report

## Related

References: SEC-0004, SEC-0003, SEC-0009, RST-0005, CHE-0105, CHE-0106, CHE-0107

## Context

CHE-0105 adopts #5 for production and names an open gap (bd adr-fmt-4yn7f G2):
no plugin/extension/isolation/capability-security ADR exists in the corpus.
SEC-0004 (Restrict Capabilities by Default) is the anchor but governs the
HOST's own authority-passing, not a loaded guest sandbox. #5 originates the
trust-boundary, capability-granting, and guest-resource-limit posture.

The WASM Component Model is capability-based by construction: a guest reaches
ONLY what the host grants as WIT imports; it has no ambient authority
(no filesystem, network, clock, or environment unless explicitly imported).
This is a STRONGER enforcement of SEC-0004:R2 ("authority passed as explicit
parameters, never globals") — the sandbox makes ambient authority structurally
impossible for the guest, not merely discouraged. SEC-0004 is an ALLY, and #5
must define the concrete grant model on top of it.

## Decision

Establish the capability and trust model for loaded domain-app WASM
components. This is gate G2 for CHE-0105.

R1 [7]: A loaded domain-app guest component has NO ambient authority. It may
  reach ONLY the WIT interfaces the host explicitly grants as imports. No
  filesystem, network, clock, random, or environment access is available to
  the guest unless the host passes it as an explicit WIT capability import
  (the WASM-native form of SEC-0004:R2, "never globals").

> **R1 clarification — declared imports vs. granted capability** (spike
> gotcha G4, bd adr-fmt-u73ej): a wasip2 guest linking libstd will show a
> non-empty DECLARED WIT import list — measured at 14 WASI imports (3
> `wasi:io/*`, 1 `wasi:clocks/monotonic-clock`, 10 `wasi:cli/*`; bd
> adr-fmt-z2hoi Q1) — as a TOOLCHAIN-LEVEL artefact universal to every
> `wasm32-wasip2` Rust libstd binary (byte-identical to a bare
> hello-world component), affecting every future WASM guest host in this
> codebase, not a grant and not specific to any one guest. "Zero
> declared imports" is NOT achievable with today's wasip2 libstd and is
> NOT what R1 asserts. R1's
> property is zero GRANTED capability: the host's linker decides what is
> actually satisfiable at instantiation, and a declared import the host does
> not satisfy is not a capability, nor is one satisfied only by a trapping
> stub that conveys no authority. Conformance to R1 is therefore asserted
> against the HOST's linker configuration, not against the guest's declared
> import list — a guest with a non-empty declared-import list and an empty
> host-satisfied set is R1-conformant, not a violation.

R2 [7]: The host grants the MINIMAL capability set a domain app needs to run
  domain LOGIC: the domain-event/command marshalling seam (CHE-0106:R4) and
  nothing more by default. Substrate-port authority (`EventStore`/
  `CommandGateway`) is NOT granted to the guest — the guest cannot reach the
  event store or command gateway directly; those stay host-side (CHE-0105:R2,
  preserves CHE-0005). Any additional capability is opt-in and ADR-justified.

R3 [5]: Guest resource consumption is BOUNDED per SEC-0003 (availability):
  wasmtime fuel or epoch-interruption limits, memory limits, and a store size
  cap are configured on every guest instance. An unbounded or unconfigured
  guest instance is forbidden — a loaded component MUST run under explicit
  execution and memory bounds.

R4 [5]: The trust boundary is the host/guest membrane. Data crossing it is
  copy-by-value (serde-marshalled per CHE-0010; no shared memory), and the host
  validates guest-produced values at the boundary before acting on them
  (SEC-0002 integrity-at-trust-boundaries applies: the guest is untrusted
  input to the host).

R5 [5]: Guest component supply chain is governed: a loaded `.wasm` component
  is a dependency subject to SEC-0009 (dependency auditing) posture — provenance
  is recorded, and only components from a controlled build path are loaded.
  Arbitrary/untrusted third-party components are NOT loaded in production
  without a dedicated ADR extending this trust model.

R6 [4]: The wasmtime host embedding preserves `#![forbid(unsafe_code)]`
  (RST-0005:R1) if the safe wasmtime API suffices (CHE-0105:R6); any local
  unsafe in the host requires the paired exception ADR per RST-0005:R2 /
  SEC-0004:R4. wasmtime's internal unsafe is dependency unsafe (RST-0005:R3).

## Consequences

+ becomes easier: the guest sandbox structurally enforces SEC-0004's
  no-ambient-authority thesis — the guest cannot even name authority it was
  not granted.
+ becomes easier: substrate isolation is explicit — the guest provably cannot
  reach the event store, reinforcing CHE-0005 at the security layer.
- becomes harder: every capability a domain app needs must be modelled as an
  explicit WIT import and justified — no convenient ambient escape hatch.
- becomes harder: guest supply-chain governance (R5) adds a provenance/audit
  obligation that in-process compile-time crates did not carry.
risks/migration: additive — originates the plugin trust model the corpus
  lacked. Gate G2 for CHE-0105; no code lands before CHE-0105:R3 is satisfied.
