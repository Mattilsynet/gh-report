# CHE-0106. WASM Gate G1: Crate-DAG to WASM-Component Correspondence

Date: 2026-08-01
Last-reviewed: 2026-08-01

Tier: B
Status: Accepted
Crates: gh-report, cherry-pit-core

## Related

References: CHE-0105, CHE-0029, CHE-0030, CHE-0005, CHE-0018, RST-0005, SEC-0004

## Context

CHE-0105 adopts #5 for production but names an open gap (bd adr-fmt-4yn7f G1):
no ADR maps a RUNTIME component boundary onto the compile-time crate DAG.
CHE-0029 (Cargo workspace, acyclic crate DAG) governs COMPILE-TIME topology
only; a WASM component graph is a second, orthogonal axis the corpus has not
addressed. Without a correspondence rule, the runtime component graph could
silently diverge from — or re-introduce cycles the crate DAG forbids into —
the compile-time graph.

Two axes must be kept distinct and related: the compile-time crate DAG
(CHE-0029:R1, acyclic; CHE-0029:R4 keeps `cherry-pit-core` a leaf with
zero transport/runtime/filesystem deps) and the runtime component graph
(host process + loaded guest components communicating over WIT interfaces).
The wasmtime host embedding is a runtime dependency and MUST NOT land in
`cherry-pit-core` (CHE-0029:R4).

## Decision

Establish the correspondence rule between the compile-time crate DAG and the
runtime WASM component boundary. This is gate G1 for CHE-0105.

R1 [5]: The wasmtime host embedding lives in a dedicated host/adapter crate,
  NOT in `cherry-pit-core`. CHE-0029:R4 (core = pure-domain deps only, zero
  transport/runtime/filesystem, stays a leaf) is preserved; the host crate is
  a new node in the crate DAG that depends on core, never the reverse.

R2 [5]: Each WASM guest domain-app component corresponds to a compile-time
  crate boundary that is already a node (or leaf) in the CHE-0029 acyclic DAG.
  The runtime component graph MUST NOT introduce a dependency edge that the
  compile-time crate DAG forbids: if crate A does not depend on crate B in the
  DAG, A's guest component must not gain a WIT import edge to B's component.

R3 [5]: The runtime component graph is acyclic, mirroring CHE-0029:R1. A guest
  component may import host-provided WIT interfaces (capabilities, per SEC-0013)
  and export its domain-logic interface; guest-to-guest cycles are forbidden by
  construction, as is any component edge that would form a cycle when overlaid
  on the crate DAG.

R4 [4]: The WIT interface package defining the host/guest seam is owned by the
  host/adapter crate (R1), not by `cherry-pit-core`. The domain-logic types
  crossing the seam are the serde-serializable events/commands (CHE-0010,
  CHE-0014 per CHE-0105:R5); `cherry-pit-core`'s Rust surface (CHE-0030 flat
  public API) is unaffected — the component boundary is defined by WIT, not by
  Rust module paths.

R5 [4]: A guest component maps to domain LOGIC only (CHE-0105:R1); it never
  corresponds to a substrate-port crate. The `EventStore`/`CommandGateway`
  implementation crates stay host-side of the boundary and are never compiled
  to, nor selected by, a guest component (preserves CHE-0005; CHE-0105:R2).

## Consequences

+ becomes easier: the two graphs have an explicit correspondence — reviewers
  can check a runtime component edge against the crate DAG and reject
  divergence mechanically.
+ becomes easier: `cherry-pit-core` purity (CHE-0029:R4) is protected by
  construction — the host embedding has a named home outside core.
- becomes harder: adding a guest component now requires confirming its edges
  against the crate DAG; a component that "wants" an edge the DAG forbids is a
  signal the crate DAG itself needs an (ADR-governed) change first.
risks/migration: additive — introduces a host/adapter crate and a WIT package;
  no existing crate's dependencies change. Gate G1 for CHE-0105; no code lands
  before CHE-0105:R3 is satisfied.
