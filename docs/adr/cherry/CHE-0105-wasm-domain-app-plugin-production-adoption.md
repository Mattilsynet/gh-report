# CHE-0105. WASM Domain-App Plugin: Production Adoption (Supersedes CHE-0104 Spike-Only Ratification)

Date: 2026-08-01
Last-reviewed: 2026-08-01

Tier: B
Status: Accepted
Crates: gh-report, cherry-pit-core

## Related

References: CHE-0005, CHE-0072, CHE-0010, CHE-0018, CHE-0014, PGN-0010, SEC-0004, RST-0005, CHE-0106, SEC-0014, CHE-0107 | Supersedes: CHE-0104

## Context

CHE-0104 ratified #5 (WASM domain-app plugin) as gated, throwaway-spike-only
R&D: GATE0 was resolved favourable (a CHE-0005:R1-compliant design exists) but
adoption was withheld pending three new ADRs (G1/G2/G3), a pre-1.0 toolchain,
and spike data. The user has now authorized #5 as FULL PRODUCTION adoption.
This ADR supersedes CHE-0104's spike-only ratification; CHE-0104 moves to
`docs/adr/stale/` per the corpus supersession convention.

The crux (bd adr-fmt-4yn7f, CHE-0104 GATE0) must be re-affirmed at production
strength, not merely restated. Two designs sit on opposite sides of a
Tier-S line:

- COMPLIANT: put domain LOGIC (`Aggregate::apply`, `HandleCommand::handle`,
  `Policy::react`) behind a compile-time-typed WIT host seam. The substrate
  ports (`EventStore`, `CommandGateway`) stay host-side, associated-type-bound,
  object-unsafe, never crossed by the guest. One concrete `EventStore`/
  `CommandGateway` type survives per bounded context. This is the CHE-0072
  precedent (startup backend selector ruled CHE-0005:R1-compliant because one
  concrete `EventStore` type survives) and the PGN-0010 template (sealed typed
  seam, opaque handles, sync facade over internal async).
- FORBIDDEN: runtime-SELECT or type-erase the substrate ports themselves —
  a WASM guest that supplies or swaps the `EventStore`/`CommandGateway`
  implementation. This overturns CHE-0005 (Tier S: "make illegal
  architectures fail to type-check; composition is compile-time; NO RUNTIME
  IMPLEMENTATION SELECTION") and would require a Tier-S supersession this ADR
  does NOT perform and does NOT authorize.

Production adoption changes the enforcement weight and removes the spike gate;
it does NOT move the line. The toolchain floor is real (bd adr-fmt-if886:
wasmtime v47, WASI 0.2 stable since 2024-01-25, `wasm32-wasip2` stable Rust
since 1.82; `cargo-component`/`wit-bindgen` pre-1.0) and is admitted as
production risk under RST-0002's conservative-adoption posture, not as a
blocker.

## Decision

Ratify #5 (WASM domain-app plugin) as PRODUCTION adoption for gh-report.
The design sits on the COMPLIANT side of the CHE-0005 line: domain LOGIC
behind a typed WIT host seam; substrate ports stay host-side. This ADR
supersedes CHE-0104's spike-only ratification.

R1 [5]: #5 is adopted for production. The guest WASM component hosts domain
  LOGIC ONLY (`Aggregate::apply`/`HandleCommand::handle`/`Policy::react`
  equivalents behind a WIT interface). The substrate ports (`EventStore`,
  `CommandGateway`) remain host-side, associated-type-bound, object-unsafe,
  and are NEVER supplied, selected, or erased by the guest.

R2 [5]: This ADR does NOT overturn CHE-0005 (Tier S). One concrete
  `EventStore`/`CommandGateway` type survives per bounded context, wired at
  compile time host-side; the guest boundary is a typed domain-logic seam,
  not a substrate-port selector. Any design that runtime-selects or type-erases
  a substrate port is OUT OF SCOPE here and would require a separate Tier-S
  supersession of CHE-0005 — which this ADR neither performs nor authorizes.

R3 [5]: Production adoption is contingent on the three gate ADRs being
  Accepted and satisfied: CHE-0106 (crate-DAG-to-component correspondence, G1),
  SEC-0014 (plugin/capability/trust model, G2), CHE-0107 (async-in-components
  posture, G3). This ADR is ratified jointly with them; no #5 production code
  lands before all three are in force.

R4 [4]: #5 RIDES the corpus invariants the CHE-0104 analysis identified as
  enablers: CHE-0010 (events already serde-serializable — the exact
  copy-by-value property the host/guest membrane needs; the WASM marshalling
  codec becomes a new concrete R3-consumer), CHE-0018 (sync domain aligns with
  the synchronous WIT guest boundary; async stays host-side), PGN-0010 (sealed
  typed seam, opaque handles, sync facade over internal async — the shape to
  imitate).

R5 [4]: #5 AMENDS CHE-0014 within its stated latitude: commands that cross the
  host/guest membrane opt into `#[derive(Serialize, Deserialize)]` per-command.
  CHE-0014's trait default (commands not serializable by default) is UNCHANGED;
  no supersession of CHE-0014 is performed.

R6 [4]: The unsafe-code posture is resolved before production code per
  RST-0005:R2 / SEC-0004:R4: either prove the wasmtime host embedding uses only
  the safe wasmtime Rust API and keep `#![forbid(unsafe_code)]` (preferred), or
  ship a paired unsafe-exception ADR with a safety-proof sketch. wasmtime's own
  internal unsafe is DEPENDENCY unsafe, governed by RST-0005:R3 (minimize,
  cargo-geiger), not by R1's `#![forbid]` in workspace crates.

R7 [3]: No code accompanies this ADR. Production build is authorized in
  principle but sequenced behind the gate ADRs (R3) and the unsafe-posture
  resolution (R6); this ADR is the binding rationale record for the production
  decision and the crux resolution.

## Consequences

+ becomes easier: #5 is now a first-class production track, not R&D — the
  spike gate and throwaway-only constraint from CHE-0104 are removed.
+ becomes easier: the crux is settled at production strength — domain logic
  behind a typed seam is the sanctioned shape, and the forbidden
  substrate-selection shape is named explicitly so no future increment drifts
  into it by accident.
- becomes harder: the CHE-0005 line is now load-bearing in production code —
  any pressure to let a guest supply a substrate port is a Tier-S
  supersession, not an incremental change, and must be escalated as such.
- becomes harder: production adoption inherits pre-1.0 toolchain risk
  (`cargo-component`/`wit-bindgen`); RST-0002 conservative-adoption discipline
  and version pinning become operationally load-bearing.
risks/migration: supersedes CHE-0104 (spike-only) — CHE-0104 moves to
  `docs/adr/stale/`. No substrate-port contract changes; the guest seam is
  additive above the existing compile-time substrate. Rollback is removal of
  the host seam and guest components, not a substrate change.
