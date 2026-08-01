# CHE-0104. Domain-App Boundary: Process-Split (#8) and WASM-Plugin (#5) Are Composable, Not Competing

Date: 2026-08-01
Last-reviewed: 2026-08-01

Tier: B
Status: Accepted
Crates: gh-report, cherry-pit-wq, pardosa-nats

## Related

References: CHE-0005, CHE-0072, CHE-0010, CHE-0018, CHE-0014, CHE-0048,
CHE-0088, PGN-0010, PGN-0016, PGN-0022, PGN-0023, PGN-0024, COM-0018,
GND-0010, SEC-0004, RST-0005

## Context

Roadmap Phase 2 framed two gh-report domain-app boundary mechanisms as
COMPETING and requiring a joint decision: #8 (operational split — collector
process moves to `pardosa-nats` JetStream transport plus a separate serving
process) and #5 (WASM domain-app plugin — load domain logic through a
`wasmtime` host seam). Oracle analysis (adr-fmt-njzyb, inputs: bd
adr-fmt-pq1b6.3.2, adr-fmt-pq1b6.3.3, adr-fmt-4yn7f WASM deep-dive,
adr-fmt-if886 WASM tech review, adr-fmt-pq1b6.1.1 STPA DR-1..DR-10)
resolves the framing: the two arms are composable, and #8 is a
prerequisite-enabler of #5.

GATE0 crux (from bd adr-fmt-4yn7f, oracle adr-fmt-njzyb §0): does a WASM
boundary put domain LOGIC behind a typed host seam (potentially
CHE-0005-compliant, precedent CHE-0072), or does it runtime-SELECT the
substrate ports themselves (`EventStore`/`CommandGateway`), which would
overturn CHE-0005 (Tier S) and require supersession? Resolution: a
CHE-0005:R1-compliant #5 exists as a design constraint — keep one concrete
`EventStore`/`CommandGateway` type per bounded context, load only domain
logic (`Aggregate::apply`/`HandleCommand::handle`) behind a compile-time
typed WIT host seam; substrate ports stay host-side, never crossed by the
guest. CHE-0072 (Tier B, template PGN-0010) already ruled a startup backend
selector CHE-0005:R1-compliant on exactly this test — one concrete
`EventStore` type survives. No Tier-S supersession is required; #5 remains
gated on new-ADR surface and toolchain maturity, not on paradigm.

## Decision

Ratify: #8 (process-split) and #5 (WASM domain-app plugin) are COMPOSABLE,
not mutually exclusive; #8 is a prerequisite-enabler of #5. Adopt the
recommended default PREREQ-then-#8, with #5 deferred as gated, spike-only
R&D.

R1 [5]: GATE0 is resolved favourable for #5: a CHE-0005:R1-compliant design
  exists (domain logic behind a typed WIT host seam; substrate ports stay
  host-side, one concrete `EventStore`/`CommandGateway` type per bounded
  context; precedent CHE-0072, template PGN-0010). No Tier-S supersession
  of CHE-0005 is required by adopting #5 in this shape. The
  runtime-select-substrate-ports design that WOULD overturn CHE-0005:R1 is
  not the design #5 must adopt, and this ADR does not authorize it.

R2 [5]: #8 and #5 are COMPOSABLE on different axes, not competing
  alternatives requiring a single choice: #8 is process TOPOLOGY (WHERE the
  collector/serving loop runs — split into a separate serving process fed
  via `pardosa-nats` JetStream); #5 is a composition-ISOLATION seam (HOW
  domain logic is admitted into a host process). #8's serving-process async
  host edge (GND-0010, PGN-0010 sync-facade-over-async) is the same host
  edge into which a #5 WASM guest would later slot as the synchronous
  domain call. Choosing #8 does not foreclose #5; it de-risks it by
  building #5's eventual host substrate first.

R3 [4]: Recommended default sequencing: PREREQ (serializable commands) →
  #8 (near-term, contingent) → external WASM-toolchain maturation → #5
  spike → GATE1. The shared PREREQ de-risks both arms and ships regardless
  of which arm follows. #8 is adopted CONTINGENT on clearing three STPA
  gates (R4); #5 stays parked at GATE0-cleared/pre-GATE1, throwaway-spike
  only, no crate restructuring before the spike returns data.

R4 [5]: #8 is gated HARD on three STPA-derived gates (bd
  adr-fmt-pq1b6.1.1), and MUST NOT proceed to build before they clear:
  - DR-10: verify PGN-0022:R1 multi-process overlap-detection emission
    actually fires before splitting the collector into two writers
    (independent check: bd #10-FizzBee). THE #8 blocker.
  - DR-2: `DEFAULT_LOCK_TTL`(900s) == `COLLECTION_INTERVAL`(900s) coupling
    becomes a live trigger once concurrent instances are the normal case,
    not the exception; widen the TTL or split the run before relying on
    the lock alone.
  - DR-3: PGN-0023 lag ceiling is unratified; cross-process projection lag
    becomes real under #8 and must be enforced or disclosed.

R5 [4]: #5 requires originating three new ADRs before any non-throwaway
  code, plus a pre-1.0 toolchain (`cargo-component`/`wit-bindgen`) and
  spike-only validation (zero event-sourced-DDD-plugin precedent exists,
  per bd adr-fmt-if886):
  - G1: crate-DAG-to-runtime-component map.
  - G2: plugin/capability/trust boundary.
  - G3: async-in-WASM-components stance.
  Additionally, #5 must resolve an unsafe-code posture (prove a
  safe-only wasmtime host API and keep `forbid(unsafe_code)`, or a paired
  exception ADR per RST-0005:R2/SEC-0004:R4) before any non-throwaway code.

R6 [4]: Both arms overturn zero binding ADRs as scoped here. #8 rides
  substrate the corpus already anticipates: COM-0018 (single-writer
  fencing, R5 applies), GND-0010 (PC/EC), PGN-0016 (subject-scoping OCC
  fence), PGN-0022/PGN-0023 (SATISFY + the two gaps named in R4),
  PGN-0024 (scaling, RIDE), CHE-0048 (single-proc projection lock, R7
  per-aggregate), CHE-0088 (writer-transfer fencing). #5 RIDES CHE-0010
  (events already serde), CHE-0018 (sync domain / async host), PGN-0010
  (sealed-seam template), SEC-0004 (WASM capability isolation is a
  no-ambient-authority upgrade, not a weakening); #5 AMENDS CHE-0014 within
  its stated latitude — command-crossing apps opt into serde derives
  per-command; CHE-0014's trait default (commands not serializable by
  default) is unchanged.

R7 [3]: No code accompanies this ADR. This is deliberately ADR-only,
  mirroring the CHE-0103 pattern: exercising either arm's scaffolding
  before its respective gates (R4 for #8, R5 for #5) clear would validate
  behaviour under forbidden conditions. The build items — #8
  (bd adr-fmt-pq1b6.3.2) and #5 (bd adr-fmt-pq1b6.3.3) — REMAIN OPEN,
  pending user go/no-go on the XL/irreversible build; this ADR ratifies the
  decision and rationale only.

## Consequences

+ becomes easier: the fork is no longer framed as an either/or paradigm
  choice; #8 can proceed on its own bounded, closeable risk (three STPA
  gates) without waiting on #5's toolchain maturity or vice versa.
+ becomes easier: #5's eventual host substrate (async serving-process
  edge) gets built as a side effect of adopting #8, reducing #5's future
  spike cost.
− becomes harder: sequencing discipline is now load-bearing — committing
  #5 crate restructuring before its throwaway spike returns data, or
  building #8 before DR-10/DR-2/DR-3 clear, reintroduces the exact risks
  this ADR resolves them against.
risks/migration: additive-decision only — no code changes, no
  supersession. #8 and #5 build items stay open pending user go/no-go;
  this ADR is the binding rationale record for whichever the user
  authorizes next, and for the composable (not competing) framing itself.
