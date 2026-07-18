# CHE-0094. cherry-pit-sd-viz Design

Date: 2026-07-18
Last-reviewed: 2026-07-18

Tier: B
Status: Accepted

## Related

References: CHE-0029, CHE-0084:R5, CHE-0007, CHE-0086, CHE-0055, CHE-0010, COM-0014

## Context

`gh-report-queue-viz` (adr-fmt-t63uo mission; topology grounded in
adr-fmt-223sd) was a standalone, client-side, animated discrete-event
simulation of gh-report's runtime queue network, living inside the
`gh-report` application tier despite mirroring generic cherry-pit substrate
shapes (`WorkQueue`, `BatchTracker`, `CachedPage`/`CachedBody`,
`PageUpdateEvent`, `ArcSwap` generation publishing) rather than gh-report
vocabulary. Per CHE-0086's rejected-alternative reasoning ("a new crate would
create another governance island without improving the CHE-0029 DAG"), an
un-ratified crate addition is precisely the move that reasoning warns
against; the crate's materiality (a new systems-dynamics modelling paradigm
alongside the existing discrete-event core, plus a wasm32 view target) and
the precedent that every material cherry-pit-* mechanism crate carries a
dedicated design ADR (CHE-0053 storage, CHE-0049 web, CHE-0055 wq, CHE-0051
agent, CHE-0087 leptos) converge on ADR-required rather than optional.

The crate is re-homed to the cherry-pit domain as `cherry-pit-sd-viz`,
ratifying it as a governed member rather than an island. The standing
cherry-pit→pardosa severance is enforced by CHE-0029 (acyclic DAG + core
dependency budget) and CHE-0084:R5 ("MUST NOT introduce any
cherry-pit->pardosa dependency edge") — **not** CHE-0010, which governs
`DomainEvent` supertrait bounds and is only contextually relevant to
pardosa-adjacent type placement (CHE-0086:R10). `cherry-pit-sd-viz` mirrors
`pardosa-nats` type *shapes* (`JetStreamAckPosition`, `JetStreamAppendAck`,
`JetStreamBackend`) in source-level doc-comments only; it carries zero
pardosa Cargo dependency, and that absence is the load-bearing severance
proof this ADR ratifies (CHE-0055:R7 "absence is load-bearing" precedent).

## Decision

`cherry-pit-sd-viz` ships as a single crate covering both the discrete-event
simulation core (host-testable, `sim` module) and its wasm32-only browser
view (`view` module), re-homed unchanged in behaviour from
`gh-report-queue-viz`. No cherry-pit-* crate depends on it; it does not
touch `cherry-pit-core`'s dependency budget. The rename is structural only —
this ADR ratifies the crate's identity and dependency posture, not any
change to simulation semantics.

R1 [5]: crate name is `cherry-pit-sd-viz`, a single crate (not split into
`cherry-pit-sd` + `cherry-pit-sd-viz`) — the host-testable sim core and the
wasm32-only view share one dependency-gated crate via
`#[cfg(target_arch = "wasm32")]`, keeping host consumers wasm-dep-free
without a second crate; name covers both the systems-dynamics/discrete-event
modelling layer and the viz/wasm view per CHE-0055:R2 precision-naming
convention.

R2 [5]: dependency set is enumerated explicitly: `leptos`, `wasm-bindgen`,
`web-sys`, `js-sys`, `any_spawner` — all gated under
`cfg(target_arch = "wasm32")` / `target.wasm32-unknown-unknown`. The
dependency set MUST contain NO `pardosa*` crate; this absence is
load-bearing (CHE-0055:R7 precedent). Type shape-mirroring of
`pardosa-nats` concepts (`JetStreamAckPosition`, `JetStreamAppendAck`,
`JetStreamBackend`) is source-level doc-comment mirroring only, never a
Cargo dependency edge (CHE-0029 acyclic DAG + CHE-0084:R5).

R3 [5]: crate is a leaf consumer; no `cherry-pit-*` crate may depend on it
(CHE-0029:R1 acyclic); it does not enter `cherry-pit-core`'s dependency
budget (CHE-0029:R4 keeps core = serde/uuid/jiff only).

R4 [5]: `#![forbid(unsafe_code)]` at the crate root (was
`#![deny(unsafe_code)]`); no `unsafe` blocks, `unsafe impl`, or `unsafe fn`
bodies (CHE-0007:R1/R2/R3). The wasm view uses safe `wasm-bindgen`/`web-sys`
abstractions only.

R5 [4]: the wasm32-only view (`view` module) is gated behind
`#[cfg(target_arch = "wasm32")]`; the discrete-event sim core (`sim` module)
stays host-testable and wasm-dep-free at the type level, matching
CHE-0029:R5 adapter posture and the CHE-0055 orthogonal-dependency-profile
reasoning.

R6 [4]: the discrete-event core and any future systems-dynamics
(Stock/Flow/Converter/Connector/feedback) layer are sibling surfaces within
this crate, never folded into a unified type (CHE-0086:R3 sibling-surface
discipline); public API is a flat re-export per CHE-0030:R1.

R7 [4]: gh-report-specific vocabulary (`WebhookKind`, `SweepPhase`,
`EvidenceProjectionEvent`, `EvidenceProjection`, `StreamLog`, `MemoBuilder`,
`UpdatedAt`, `BaselineDecision`, `InventoryOutcome`, `InventoryGate`,
`PardosaBackend`, `NativeStore`, `DurableStore`, `BudgetGate`) that entered
`sim.rs` under the crate's prior gh-report-domain framing is grandfathered
as descriptive doc-comment provenance for this structural rename only;
future additions of org/GitHub/report vocabulary into this crate are subject
to the CHE-0084:R1 vocabulary test and MUST NOT enter without a follow-up
ADR justification.

R8 [5]: CI tripwire `deny pardosa deps in cherry-pit-sd-viz`
(`.github/workflows/ci-reusable.yml`) asserts `cargo tree -p
cherry-pit-sd-viz` resolves zero `pardosa*` crates, mirroring the existing
async-trait tripwire shape (CHE-0029:R6 pattern). This is the load-bearing
severance check.

R9 [4]: `#[non_exhaustive]` on error enums plus additive-only minor
discipline applies from v0.1 (CHE-0021, CHE-0022:R1, CHE-0055:R16).

R10 [4]: tests follow the CHE-0038 taxonomy — unit tests beside their
modules (`sim.rs`'s existing 23-test `#[cfg(test)]` module, unchanged by
this rename); the wasm view is tested per its target harness when such
tests are added.

## Consequences

**Positive.** The crate becomes a ratified cherry-pit-domain member rather
than a governance island; the pardosa-severance posture is now CI-enforced
rather than merely observed; the name precisely covers both modelling
surfaces the crate carries.

**Negative.** One-shot rename churns `Cargo.toml`, workspace member lists,
CI tripwire enumeration, and committed wasm `pkg/` artefact filenames;
`README.md`/`bootstrap.js`/`index.html` references must track the new
crate/package name.

**Open / deferred.** The systems-dynamics (STELLA
Stock/Flow/Converter/Connector/feedback) modelling layer itself is out of
scope for this ADR — this ADR charters the crate identity and dependency
posture only; the modelling layer's design lands in a follow-up mission
(cpsdviz-02-sdcore) and, if materially novel, its own ADR amendment.

## Rejected Alternatives

**Two-crate split: `cherry-pit-sd` (host-testable core) + `cherry-pit-sd-viz`
(wasm view, depends on core).** Rejected for this rename: the sim core is
small enough, and its wasm-dep isolation is already achieved via
`#[cfg(target_arch = "wasm32")]` module gating rather than crate boundary,
so a second crate would add DAG surface (CHE-0029 acyclic bookkeeping) and
governance overhead without a compensating benefit at this scale. If the
sim core grows independently reusable consumers that must not pull
`wasm-bindgen`/`web-sys` transitively, a future ADR may split the crate;
this ADR does not foreclose that path.

**Cite CHE-0010 as the pardosa-severance authority (as the originating
mission brief proposed).** Rejected: CHE-0010 governs `DomainEvent`
supertrait bounds, not the cherry-pit→pardosa dependency-edge prohibition.
The correct severance authority is CHE-0029 (acyclic DAG + core dependency
budget) and CHE-0084:R5 (explicit prohibition on introducing a
cherry-pit→pardosa edge). CHE-0010 is cited here only contextually, per the
oracle correction (adr-fmt-zh20j).

## Amendment 2026-07-18 (SD connection grammar)

Per the l.125-129 deferral above, the systems-dynamics modelling layer's
design lands here rather than as a new ADR: oracle review (adr-fmt-sxlt8)
ruled the grammar amends CHE-0094 in place, since CHE-0086's materiality
bar is cleared but the l.128-129 clause already pre-commits deferred SD
work to "its own ADR amendment" — not a freestanding ADR. Source grammar:
adr-fmt-qaavg (directed-connection matrix + 8 invariants, Stock/Flow/
Converter/Cloud vocabulary per adr-fmt-0pe95).

R11 [4]: the following connection invariants are BINDING for
`cherry-pit-sd-viz`'s systems-dynamics layer (CHE-0086:R3 sibling-surface
per R6 above), enforced by the type layer — compile-time endpoint-kind
typing plus a validated builder that rejects illegal edges at
construction, not merely by convention:

1. A flow's material endpoints are drawn from `{Stock, Cloud}` only, on
   both ends; never a Converter.
2. Connectors carry information only; material moves exclusively through
   flows.
3. A stock's value changes only via its attached flows; nothing else may
   write to a stock.
4. A connector's HEAD may point only into a Flow's rate equation or a
   Converter's equation — never into a Stock or a Cloud.
5. A connector's TAIL may originate only from a Stock, a Flow, or a
   Converter — never from a Cloud (clouds carry no readable value).
6. Cloud is a flow-endpoint only; it is never a connector endpoint (head
   or tail).
7. A converter can never sit in a flow's material path — it informs a
   flow's rate equation via connector, but holds no pipe of its own.

R12 [3]: invariant 8 (every feedback loop must pass through at least one
Stock; a stock-free loop is a degenerate algebraic loop) is ADVISORY and
documented only this iteration, not type-layer-enforced. adr-fmt-qaavg
flags this as its weakest-sourced claim — no primary Vensim/Sterman
citation was reached for it, only structural inference. Active loop-path
enforcement (graph traversal proving every cycle threads a Stock) is
deferred to a follow-up mission; until then a stock-free loop is a
lint-level warning at most, not a build-time rejection.

R13 [5]: the SD-layer's error type, `SdConnectionError`, MUST be
`#[non_exhaustive]` from v0.1 (CHE-0021, CHE-0094:R9) — additive variants
for new invariant violations are non-breaking.

NOTE (2026-07-18, provenance only — not a new rule): mission
sd-ghreport-reconcile (epic adr-fmt-odlad), oracle ruling adr-fmt-7kdrt
(NOTE-ONLY). `binding.rs::tier1_model()` (source adr-fmt-vrycy) is the
first canonical full gh-report Tier-1 model instance built via the R11
grammar above — 7 stocks / 5 clouds / 3 converters / 11 flows / 8
connectors, legal under the validated builder. The wasm32 view (R5)
gained a reusable SD-element component-template family: 5 concrete
sibling templates plus one non-SD overlay (`overlay.rs::SweepPhaseBadge`),
kept flat per R6, with host-pure layout math in `layout.rs`.
`SweepPhaseBadge` is scoped explicitly as an annotation-only overlay — it
is not an `sd::Model` node and carries no `sdt-*` styling.
</content>
