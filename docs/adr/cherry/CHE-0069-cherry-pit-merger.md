# CHE-0069. cherry-pit-merger — Command-Side Single-Task Serialiser

Date: 2026-06-10
Last-reviewed: 2026-06-10
Tier: B
Status: Accepted
Crates: cherry-pit-merger

## Related

References: CHE-0005, CHE-0017, CHE-0029, CHE-0030, CHE-0024, CHE-0042, CHE-0073

## Context

CHE-0073 collapses gh-report's durable surface to the Repo aggregate, while preserving the command-side need that CHE-0054 first exposed: two concurrent same-`domain_key` callers could both observe `lookup → None`, both call `EventStore::create`, and produce an orphan stream the routing index never points back to. gh-report closes the window structurally with a single-task `Merger` consuming `mpsc<MergerCommand>`. Lifting this primitive into a shared crate is the canonical answer — every future cherry-pit consumer needs the same loop with the same TOCTOU guarantee against its own aggregates.

## Decision

cherry-pit-merger is a new sibling crate of cherry-pit-app exposing a generic `Merger<A, S, B, Arm>` plus a `MergerArm<A>` trait carrying the caller's pure per-command handler. The crate depends only on `cherry-pit-core` (and `tokio`), keeping the acyclic DAG and letting any consumer wire it without picking up the agent surface. The merger task owns the sole `EventStore` write handle; consumers dispatch through a cheaply-clone-able `MergerHandle` whose `dispatch` returns `Result<(), Arm::Err>` for `.await? -> Result<…>` ergonomics.

R1 [5]: cherry-pit-merger depends only on `cherry-pit-core` and `tokio` (per CHE-0029 acyclic DAG); it is a sibling of cherry-pit-app rather than a downstream, so consumers may wire it without depending on the full agent composition surface, and the `MergerArm` trait stays parameterised on `A: Aggregate` per CHE-0005:R1 single-aggregate-per-port

R2 [5]: The `MergerArm<A>` trait carries three pure methods — `persist_mode(&cmd) -> PersistMode`, `handle(&A, cmd) -> Result<Vec<A::Event>, Err>`, `publish_label(&cmd) -> &'static str` — plus a default-impl `missing_key_error(&str) -> Err`; consumers write the exhaustive command-matcher inside `handle` (the CHE-0017:R2 caller-writes-the-matcher pattern), and the merger crate stays aggregate-agnostic

R3 [5]: `PersistMode` is a three-variant enum (`Create` for fresh-per-call, `CreateOrAppend(key)` for lazy create-or-append by domain key, `AppendStrict(key)` for must-exist) corresponding to the three persist shapes lifted verbatim from gh-report's Run/Repo/WebhookDelivery triads (CHE-0054:R1–R3) — adding a fourth persist shape requires a superseding ADR rather than an in-place extension

R4 [5]: The merger task is a single `tokio::task` consuming a bounded `mpsc::Receiver<MergerCommand<A, Arm>>` (capacity 1024); two concurrent same-key callers serialise at the channel front door so the I1 TOCTOU `lookup → create → or_insert` sequence is single-flighted structurally — exactly one `EventStore::create` per `domain_key`, no orphan stream

R5 [5]: `MergerArm::Err` carries a `From<cherry_pit_core::StoreError>` bound so the merger lifts persist-side failures (`load`, `create`, `append`, `ConcurrencyConflict`) into the arm's domain-error shape uniformly — consumers implement one `impl From<StoreError>` per aggregate-error and the merger's reply channel returns one error type end-to-end

R6 [5]: cherry-pit-merger exposes a flat public API at the crate root via `pub use` re-exports per CHE-0030:R1 (`Merger`, `MergerArm`, `MergerCommand`, `MergerHandle`, `PersistMode`, `MERGER_CHANNEL_CAPACITY`); internal modules (`command`, `arm`, `handle`, `merger`, `shared`) are private implementation detail

R7 [5]: The publish-side step calls `EventBus::publish` and absorbs `BusError` per CHE-0024:R1 via one `tracing::error!` per envelope at target `cherry_pit_merger` carrying `event_id`, `correlation_id`, `causation_id`, `aggregate_id`, the static `publish_label`, and the bus error — non-fatal, replay-recoverable per CHE-0024:R3

R8 [5]: A regression-pin proptest in `cherry-pit-merger/tests/i1_toctou_pin.rs` fans out up-to-48 concurrent same-`domain_key` dispatches and asserts exactly one routing-index entry, one aggregate id, `N` contiguous envelopes, `N` bus emissions, and tracker `= N`; the test must continue to pass against every code change that touches the merger's persist-side path

## Consequences

+ becomes easier: every cherry-pit consumer past gh-report wires the merger as a one-shot command-side primitive without re-litigating the I1 TOCTOU resolution or the load → handle → persist → publish loop; the trait forces a caller-written command matcher per CHE-0017:R2.

− becomes harder: a fourth persist shape requires a superseding ADR; `Box<dyn MergerArm>` is forbidden because the trait is bound to `A: Aggregate` per CHE-0005:R1, so per-aggregate `Merger` instances expand the type-parameter list as `App<G,S,B,P>` does per CHE-0051:R9.

risks/migration: Mission H (bd `adr-fmt-cq7vb.11`) migrates gh-report onto this primitive; until then the crate has no production consumer and its tests are the sole conformance gate. `MergerArm::missing_key_error` defaults to `StoreError::CorruptData`; richer shapes (e.g. `RunError::RoutingMiss`) override the method.
