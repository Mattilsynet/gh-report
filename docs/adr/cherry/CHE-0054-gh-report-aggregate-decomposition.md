# CHE-0054. gh-report Aggregate Decomposition

Date: 2026-05-10
Last-reviewed: 2026-05-10

Tier: B
Status: Accepted

## Related

References: CHE-0005, CHE-0004, CHE-0006, CHE-0008, CHE-0011, CHE-0017, CHE-0018, CHE-0020, CHE-0024, CHE-0042, CHE-0048

## Context

`gh-report` is a Cherry-domain binary doing scheduled GitHub sweeps plus webhook ingest. WU-6 v2 mission `adr-fmt-f6i` migrates it to the EDA + DDD + hexagonal posture of CHE-0004; Inc 1–4 landed the cross-cutting infrastructure, and Phase B' sub-mission B7' must decompose the event surface into aggregates.

The event surface splits into three disjoint write-coordination domains:

- Sweep lifecycle (`SweepStarted`/`SweepProgress`/`SweepCompleted`/`SweepFailed`/`EvidencePublished`), keyed by `run_id`.
- Repository evaluation (`RepoEvaluated`/`RepoRemoved`), keyed by `RepoIdentity`.
- Webhook ingest (`WebhookReceived`), keyed by `X-GitHub-Delivery`, write-once.

Three forces fix the shape: single-aggregate-per-port (CHE-0005:R1), caller-tracked CAS expected_sequence (CHE-0024:R3, CHE-0042:R3), and infrastructure-minted `AggregateId` (CHE-0020:R1) requiring application-boundary mapping from domain keys.

## Decision

`gh-report` decomposes into three aggregates — `Run`, `Repo`, `WebhookDelivery` — each with a dedicated ApplicationService, full command-side DDD shape (Command → Aggregate::handle → Vec<Event> → EventStore::append → EventBus::publish), and per-aggregate write coordination. Domain-key→AggregateId resolution lives in the application layer as in-memory `DashMap` indices held in `AppState`, not in the domain.

R1 [5]: The `Run` aggregate owns the sweep lifecycle and emits the five run-scoped variants `SweepStarted`, `SweepCompleted`, `SweepFailed`, `SweepProgress`, `EvidencePublished`, with invariants (a) `SweepStarted` is the first event of any `Run` instance, (b) at most one terminal event (`SweepCompleted` xor `SweepFailed`) per instance, (c) `EvidencePublished` may only follow `SweepCompleted`, and (d) `SweepProgress` may only appear between `SweepStarted` and a terminal event

R2 [5]: The `Repo` aggregate owns repository evaluation lifecycle keyed by `RepoIdentity` and emits `RepoEvaluated` and `RepoRemoved`, with invariants (a) `RepoEvaluated` may appear any number of times, (b) `RepoRemoved` is terminal, and (c) no events may follow `RepoRemoved`

R3 [5]: The `WebhookDelivery` aggregate owns a single GitHub webhook delivery keyed by the `X-GitHub-Delivery` header value and emits exactly one `WebhookReceived` event per instance, with the degenerate-aggregate shape sanctioned for write-once domains where idempotency by delivery id is the sole invariant

R4 [5]: Each aggregate has a dedicated ApplicationService — `RunService`, `RepoService`, `WebhookService` — exposing async use-case methods (e.g. `RunService::start_sweep`, `RunService::record_progress`, `RunService::complete`, `RunService::fail`, `RunService::publish_evidence`; `RepoService::record_evaluation`, `RepoService::record_removal`; `WebhookService::ingest`) that own the load→handle→append→publish triad per CHE-0008:R1 + CHE-0024:R3

R5 [5]: Domain-key→`AggregateId` resolution is held in `AppState` as three indices (`runs_by_key` keyed by sweep `batch_id`, `repos_by_key` keyed by `domain_key`, `deliveries_by_id` keyed by GitHub delivery id) and a per-aggregate `next_seq` tracker, populated **eagerly at boot from event-log replay** for every variant whose routing key material is present in the event payload — specifically `SweepStarted` → `runs_by_key`, `RepoEvaluated`/`RepoRemoved` → `repos_by_key`, and all variants → `next_seq` (max sequence per aggregate) — with a documented **lazy-fallback exception** for `deliveries_by_id` because the `WebhookReceived` payload does not carry `delivery_id` (CHE-0022:R6 forbids derived state in payloads; the delivery id lives only on the `RecordDelivery` command and `WebhookDelivery` aggregates are degenerate write-once instances per R3, so the merger command path remains the sole populator). The routing match over `DomainEvent` is exhaustive: any new variant produces a compile error in `AppState::bootstrap_replay_indices` forcing an explicit routing re-decision. ApplicationServices consult these indices before issuing `EventStore::load`. The eager-replay shape supersedes the pre-M3 lazy-population doctrine inherited from B7'a (placeholder `HashMap` shape pending B7'b's typed-`DomainKey` `DashMap` migration; substrate is `PardosaFileEventStore` per CHE-0065 and AFM-0023, not the retired `MsgpackFileStore`).

R6 [5]: Caller-tracked CAS sequence numbers are owned per-aggregate-instance by the ApplicationService, threaded through the load→handle→append cycle, and never leaked into domain types per CHE-0011:R1 + CHE-0042:R3

R7 [5]: The 14 existing publish sites in `collect.rs` (8 sites), `daemon.rs` (2 sites), and `webhook/mod.rs` (4 sites) migrate to ApplicationService method calls per the variant→aggregate map in R1–R3, with the Inc 1 helper `app::event_publisher` deleted on completion (per WU-6 v2 sub-mission B7'c)

R8 [5]: `gh-report` depends on `cherry-pit-core` for `Aggregate`, `DomainEvent`, `EventStore`, `EventBus`, `AggregateId`, and `EventEnvelope`; on `cherry-pit-projection` for `ProjectionDriver` + `FileProjectionStore` per CHE-0048; and on `cherry-pit-agent` for `InProcessEventBus` and `ProjectionDriverExt` only — no `App<...>` consumption per R10 — with `cherry-pit-runtime` not consumed at v0.1 and worker-pool harnessing remaining gh-report-internal pending a v0.2 evaluation per CHE-0052:R8

R9 [5]: Read-side concerns (HTML report generation, projection state) are out of scope for this ADR; the command side defined here produces the durable event log that future cherry-pit-projection-backed read models per CHE-0048 will consume

R10 [5]: Each ApplicationService (`RunService`, `RepoService`, `WebhookService`) owns the load→handle→append→publish triad directly against `cherry-pit-core::EventStore` and `cherry-pit-core::EventBus` instantiated with concrete adapters from `AppState`; gh-report does not implement `CommandGateway` nor consume `cherry-pit-agent::App<...>` at v0.1 (R8). Cross-aggregate reactions per CHE-0005:R3 dispatch at call-sites, deferring `Policy::react` choreography to a future saga ADR.

R11 [5]: `AggregateId(1)` is reserved as the stable identifier for the `OrgGovernance` singleton aggregate (the sole emitter of `EvidencePublished` at organisation scope, not per-`Run`). Reservation is enforced by allocation policy in `AppState`: aggregate-id generation for `Run` / `Repo` / `WebhookDelivery` instances starts at `AggregateId(2)` and never returns `AggregateId(1)`. This shields the singleton from id-collision as new aggregate kinds are added and lets `bootstrap_replay_indices` skip routing-index participation for `EvidencePublished` (singleton has no domain-key map). Future aggregate kinds requiring singleton semantics must claim a documented low-numbered id and amend this rule.

## Consequences

- Per-aggregate coordination is structural: `Run` serialises on `run_id`, `Repo` on `RepoIdentity`, `WebhookDelivery` is write-once. The single-process assumption of CHE-0006:R1 carries over unchanged.
- `WebhookDelivery` is intentionally degenerate (single event, no lifecycle): preserves the uniform append-via-aggregate posture and makes future enrichment (retry, processing lifecycle) a non-breaking R3 amendment.
- The `DashMap`-in-`AppState` index (R5) keeps domain free of identity resolution, satisfying CHE-0018:R1's sync-domain/async-infrastructure split. Indices are lazy and process-local; EventStore is authoritative on restart.
- One ApplicationService per aggregate (R4) avoids the god-service anti-pattern; cross-aggregate orchestration composes at the call-site in `collect.rs`, consistent with CHE-0017:R1.
- The 14-site mechanical migration (R7) is the work of WU-6 v2 sub-mission B7'c; this ADR fixes the destination so B7'c is purely transformational.

## Rejected Alternatives

**Single `Sweep` aggregate covering all variants** — Folding `RepoEvaluated`/`RepoRemoved` and `WebhookReceived` into the run lifecycle violates CHE-0005:R3's bounded-context separation: webhook ingest has no `run_id` and no per-run state, and repository identity outlives any single sweep. The aggregate's invariants would degenerate to "anything goes" once non-run events were admitted.

**Two aggregates (`Run` + `Repo`), webhook as adapter-only** — Treating webhook ingest as a port adapter that emits no events would lose the durable audit trail of webhook receipt, contradicting the persist-then-publish mandate (CHE-0024:R1) for any side-effecting input. The degenerate-aggregate shape (R3) preserves auditability at the cost of one extra aggregate type.

**Five aggregates (one per variant family)** — Splitting `SweepStarted`/`SweepProgress`/`SweepCompleted`/`SweepFailed`/`EvidencePublished` into separate aggregates fragments the run lifecycle invariants (R1) across multiple coordination boundaries, requiring cross-aggregate sagas (CHE-0040) for what is naturally a single state machine. The cost would not be repaid.

**Single `GhReportService`** — A unified ApplicationService surface combining all use cases concentrates unrelated invariant sets behind one type, defeating the per-aggregate coordination granularity that justifies the decomposition. Per-aggregate services (R4) align ApplicationService boundaries with aggregate boundaries, so each service owns exactly one invariant set.

**Domain-side identity resolution** — Moving the `domain_key→AggregateId` index into the domain layer would force the aggregate or an `Aggregate`-trait extension to know about identity minting, violating CHE-0020:R1. The application-layer `DashMap` (R5) keeps the domain free of infrastructure identity concerns.

**Persistent identity index** — Persisting the `domain_key→AggregateId` map to disk (e.g. a sidecar file per index) would introduce a second source of truth alongside the EventStore. The lazy in-memory rehydration model (R5) avoids dual-write hazards by treating the EventStore as authoritative and the index as a derived cache.

**Deferring the decomposition to WU-7** — Landing Inc 5 (the 14-site migration) without an aggregate decomposition would entrench the Inc 1 helper as a permanent shape, contradicting CHE-0004:R1's full-DDD posture mandate. The WU-6 v2 mission charter explicitly retains B7' as the DDD landing; deferring would supersede the mission rather than complete it.

**Three `cherry_pit_agent::App<G,S,B,P,D>` instances (one per aggregate)** — Would compose each aggregate via the cherry-pit-agent composition primitive per CHE-0051:R9's "one typed `App` per aggregate" guidance, requiring a `CommandGateway` impl per aggregate and three `App::run` lifecycles in `main`. Rejected because (a) CHE-0054:R8 scopes the v0.1 dep set to bus + projection-driver consumption only; (b) the three-aggregate decomposition's invariants (R1–R3) are equally well enforced by per-aggregate ApplicationServices without the App wrapper; (c) cross-aggregate reactions (Run→Repo) are dispatched at call-sites today, deferring policy-driven choreography until a saga-class workflow surfaces.
