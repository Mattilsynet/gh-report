# gh-report — Design

Status: Accepted (WU-6 v2 charter, A7'; post-M2 cutover rewrite; W1
truth-alignment corrigendum for §4, §5, §12).
Produced: grill-me design interview (v1, A4); WU-6 v2 charter rewrite (A7');
M2.e post-cutover rewrite reconciling §3, §4, §10, §12 with CHE-0054
(Accepted), CHE-0024, COM-0019; W1 truth-alignment corrigendum reconciling
§4, §5, §12 with the shipped CHE-0073 (gh-report Storage Remodel, Accepted)
+ CHE-0074 (gh-report Native Pardosa Store Port, Accepted) storage substrate.
Governs: WU-6 v2 rewrite of `crates/gh-report/` onto cherry-pit primitives,
projection-migrating `evidence_store` per CHE-0048 + CHE-0054.
Companion docs: `OPERATIONS.md` (operator runbook), `README.md` (DoD-4).

> **v2 posture (charter `wu6v2-charter-1778415390` + CHE-0054 ratification
> + M2 cutover).** §1, §2, §6, §9 are verbatim from v1. §3 + §4 + §10 + §12
> reflect the M2 post-cutover architecture: `EvidenceProjection` is the
> production read-model (CHE-0054:R10 read path; CHE-0048:R2 sole
> writer/reader), three ApplicationServices (`RunService`, `RepoService`,
> `WebhookService`) own per-aggregate write coordination per CHE-0054:R4,
> and bus-publish failures are structured per-envelope `tracing::error!`
> emissions (CHE-0024:R1 + R3 + COM-0019:R1/R4/R7). §7 is
> retitled (no v1 deployments exist; nothing to migrate). §8 / §11 are
> updated. The v1 §3a corrigendum and the v1 A9 projection-port narrative
> remain **withdrawn**: AggregateStore is *deleted* at B9' (not ported),
> per locked posture S5.b + Tension-2.
>
> **W1 truth-alignment corrigendum.** §4, §5, §12 below are further updated
> for the CHE-0073 (gh-report Storage Remodel, Accepted) + CHE-0074
> (gh-report Native Pardosa Store Port, Accepted) storage substrate that
> shipped after the M2 cutover text above was written: the durable
> `DomainEvent` EventStore is `crate::store::NativeStore`
> (`pardosa_fiber_store::FiberStore<DomainEvent>`, one pardosa fiber per
> repository domain key, `.pgno` container or NATS JetStream backend
> per CHE-0072), not `cherry_pit_gateway::MsgpackFileStore`.
> `MsgpackFileStore` is retained only for the non-persisted-by-CHE-0074
> scheduler and sweep-timeout event classes (`src/app/state.rs:48-51`).
>
> The legacy per-repo evidence cache (v1's mutable evidence store) was
> retained on disk (`src/app/evidence_store.rs`, held as a compile-only
> field on `EvidenceState`) through M2 for M3 (write-path removal +
> file deletion). It had **no production read or write callers**
> post-M2.cd (commits `0e11dbb`, `f5d59f3`, `4e177de`).
>
> **M3 update.** The v1 evidence store referenced above is now **deleted**
> from the source tree (M3 PACKAGE COMPLETE). The bare type name no
> longer appears in `crates/gh-report/`; subsequent paragraphs describe
> the M3-completed state in past tense.

## 1. Scope

`gh-report` collects GitHub governance data, evaluates repository-level
security controls, aggregates metrics, and serves HTML reports from an
in-memory cache. (Verbatim from `src/lib.rs` crate doc; ratified as
canonical scope statement for v0.1.)

## 2. CLI surface

Flat CLI, no subcommands. Two implicit modes gated by `--dump-baseline`:

- **Daemon mode** (default): requires `--org`; runs collect + serve loop.
- **Dump mode**: `--dump-baseline` reads `baseline.msgpack` and exits.

Frozen for v0.1 (see `src/bin/gh-report.rs`):

```
--org <ORG>                 # required for daemon mode
--no-resume                 # ignore checkpoint
--force-unlock              # break stale RunLock (one-shot only)
--store-dir <PATH>          # default: ./store
--dump-baseline             # one-shot mode
--max-workers <N>           # default: config::DEFAULT_MAX_WORKERS
--pass-threshold <PCT>      # dashboard tier
--warn-threshold <PCT>      # dashboard tier
--log-format <text|json>    # GH_REPORT_LOG_FORMAT env
```

CLI redesign (subcommands, env-only mode) is explicitly out of scope for
v0.1. Current shape is preserved 1:1 across the rewrite.

## 3. Domain model

gh-report decomposes into **three DDD aggregates** per CHE-0054
(Accepted) — `Run`, `Repo`, `WebhookDelivery` — each with a dedicated
ApplicationService and per-aggregate write coordination. The read side
is materialised by **one CHE-0048 projection** — `EvidenceProjection` —
the production read-model post-M2 cutover.

- **Three-aggregate decomposition (CHE-0054:R1+R2+R3).** The eight
  `DomainEvent` variants partition into three disjoint
  write-coordination domains:
  - **`Run`** (CHE-0054:R1) — sweep lifecycle. Owns
    `SweepStarted`, `SweepCompleted`, `SweepFailed`, `SweepProgress`,
    `EvidencePublished`. Keyed by `batch_id`. Invariants: `SweepStarted`
    first; at most one terminal event; `EvidencePublished` only after
    `SweepCompleted`; `SweepProgress` only between start and terminal.
  - **`Repo`** (CHE-0054:R2) — repository evaluation lifecycle. Owns
    `RepoEvaluated`, `RepoRemoved`. Keyed by `(org, repo)`.
    Invariants: `RepoEvaluated` any number of times; `RepoRemoved`
    terminal; nothing after `RepoRemoved`.
  - **`WebhookDelivery`** (CHE-0054:R3) — degenerate write-once
    aggregate. Owns one `WebhookReceived` per instance. Keyed by
    `X-GitHub-Delivery` header.
- **ApplicationServices (CHE-0054:R4).** `RunService`, `RepoService`,
  `WebhookService` — each owns the
  load → handle → append → publish triad against
  `cherry-pit-core::EventStore` + `cherry-pit-core::EventBus`. Wired
  on `AppState` (`src/app/state.rs:239-246`). No `CommandGateway`,
  no `cherry-pit-app::App<…>` consumption (CHE-0054:R8 + R10).
- **Projection (CHE-0048:R2 + CHE-0054:R10 read path).**
  `EvidenceProjection` (`src/projection.rs`) is the **sole writer and
  sole reader** of the gh-report read-model. `Projection::apply` is
  synchronous (CHE-0018:R1), infallible (CHE-0009), and idempotent
  over the same envelope sequence (CHE-0048:R3 + BC-v2-6). Bus-driven
  writes flow through `apply`; reads are served via the
  `EvidenceProjection` inherent API (`get`, `len`, `sorted_snapshot`)
  documented in §12. All production read sites hold the
  `Arc<Mutex<EvidenceProjection>>` via the `AppState::lock_projection`
  helper (`src/app/state.rs:311-317`).
- **Domain-key → AggregateId resolution (CHE-0054:R5).** Held in
  `AppState` as three in-memory indices (`run_index`, `repo_index`,
  `delivery_index` — `src/app/state.rs:273-277`), populated lazily on
  first reference and consulted by ApplicationServices before issuing
  `EventStore::load`. Indices are process-local; on restart the
  EventStore is the source of truth and indices repopulate as services
  are exercised.
- **`OrgGovernance` marker (historical, narrowing).** The v1
  Tension-2 single-aggregate posture is now refined: `OrgGovernance`
  persists as a zero-sized **documentary marker** in `src/projection.rs`
  pinning the singleton `ORG_GOVERNANCE_AGGREGATE_ID = 1` used by the
  projection's snapshot/checkpoint pairing. CHE-0054 reclassifies the
  write side into three aggregates while leaving the projection's
  read-model keyed by a single id at the storage layer (one snapshot
  file per org per CHE-0048:R1). This is intentional: the projection
  consumes events from all three aggregates and maintains a unified
  read-model; the aggregate boundaries govern *write* coordination,
  not read materialisation.

`DomainEvent` is the wire format published on the in-process event bus
and persisted in the EventStore; serialised via
`#[serde(tag = "type", rename_all = "snake_case")]`, `#[non_exhaustive]`.
Per CHE-0024:R1 + CHE-0048:R2 + BC-v2-2 payloads MUST carry sufficient
state for `apply` to be the sole writer of the read-model:

| Variant | Aggregate (CHE-0054) | Payload notes |
|---|---|---|
| `SweepStarted` | `Run` | metadata + correlation |
| `SweepProgress` | `Run` | metadata only (notification) |
| `SweepCompleted` | `Run` | metadata only (notification) |
| `SweepFailed` | `Run` | metadata + error category |
| `EvidencePublished` | `Run` | metadata only (downstream notification) |
| `RepoEvaluated` | `Repo` | **carries `evidence: RepositoryEvidence`** — load-bearing for projection state |
| `RepoRemoved` | `Repo` | repo identity (projection prunes its entry) |
| `WebhookReceived` | `WebhookDelivery` | metadata + raw delivery shape |

New variants are non-breaking; renames/removals are breaking (CHE-0022
holds; v1 BC-12 carries forward).

Domain modules (preserved verbatim under `src/domain/`):

| Module | Contents |
|---|---|
| `auth` | `AuthMode`, `TokenTier`, `Capability` |
| `cache` | `CachedRepoDetail` (per-repo eval cache entry) |
| `checks` | `RepositoryChecks` + 7 result types: SecurityPolicy, SecretScanning, Dependabot, BranchProtection, Codeowners (+ details), `ScoreCategory`, `CheckType` |
| `codeowners` | `ParsedCodeowners`, `CodeownersEntry`, `CodeownersTruncationReason` |
| `events` | `DomainEvent` (8 variants per table above) |
| `evidence` | `RepositoryEvidence`, `Evidence`, `AssessmentMetadata`, `LastCommitInfo` |
| `metrics` | `RateMetric`, `PolicyCounts`, `*Counts` per check, `AggregatedMetrics`, `OwnerMetrics`, `OwnerType`, `CollectionStatistics`, `SecretScanningObservability`, `RepoAlertSummary`, `OrgAlertSummary` |
| `repository` | `Repository`, `Visibility` |
| `run` | `RunMetadata`, `RunStatus` |
| `status` | `CollectionStatus` |
| `time` | timestamp helpers |

These domain modules are unaffected by the M2 cutover.

`AggregateStore<K, V, M>` (v1's in-memory aggregation primitive) and
the v1 evidence-store read-model wrapper around it were **deleted at
M3**. Pre-M3 they retained compile-only existence:

- `src/app/evidence_store.rs` was retained on disk with no production
  callers (deleted by M3).
- `EvidenceState` carried a compile-only store field
  (`src/app/evidence_service.rs:27` pre-M3) but it was never read or
  written from production code paths outside `evidence_service.rs`
  itself; M3 removed both the field and the file.

The read-model that v1 held in the legacy per-repo evidence cache is
now materialised by `EvidenceProjection` (per-repo evidence keyed by
`domain_key`, with derived metrics computed lazily at render time
per S2.H2.b at B8').

## 4. Runtime topology

gh-report v2 conforms to `cherry-pit-app::App::run` (BC-9; CHE-0051
rolled forward from v1).

- `gh-report::app::daemon::run(config)` becomes a thin wrapper that
  constructs the cherry-pit `App`, registers the bus, wires the
  EventStore, registers sub-aggregates, and delegates
  to `App::run`.
- **Wires** (per BC-v2-1, BC-v2-4..BC-v2-11, CHE-0051:R2/R5; storage
  substrate per the W1 corrigendum above, CHE-0073 + CHE-0074):
  - `crate::store::NativeStore` (`pardosa_fiber_store::FiberStore<DomainEvent>`)
    as the durable `DomainEvent` EventStore, one pardosa fiber per
    repository domain key (CHE-0074:R4), backed by a `.pgno` container
    or NATS JetStream (backend selection per CHE-0072). This supersedes
    the retired `cherry_pit_gateway::MsgpackFileStore<OrgGovernance>` /
    `PardosaEventStore` byte-adapter wiring (CHE-0074:R8). Companion
    `crate::store::NativeOrgStore` and `crate::store::NativeTeamStore`
    provide the org-fiber (CHE-0073:R8) and per-team-fiber (CHE-0073:R10,
    CHE-0089) current-state classes. `cherry_pit_gateway::MsgpackFileStore`
    remains wired only for the non-persisted-by-CHE-0074 scheduler and
    sweep-timeout event classes (`SchedulerEventStoreImpl`,
    `SweepTimeoutEventStoreImpl`, `src/app/state.rs:48-51`) — these are
    not part of the `DomainEvent` durability contract CHE-0073/74 govern.
  - `cherry_pit_app::InProcessEventBus<DomainEvent>` as the in-process
    bus (CHE-0051:R2).
  - No `ProjectionDriver` / `FileProjectionStore` is wired
    (W1 truth-alignment; withdrawn — see §12). `EvidenceProjection` is
    rebuilt in memory on every process start by folding
    `NativeStore::events()` (CHE-0073:R7); there is no
    snapshot-fast-path load.
- **`AppState` sub-aggregates and services.** Sub-aggregates retain
  their v1 shape for composition; ApplicationServices land per
  CHE-0054:R4:
  - `WebhookState` — webhook secret, replay protection, debounce.
  - `GithubState` — budget gate, rate limit, API client,
    repo detail cache.
  - `EvidenceState` — HTML cache, WebSocket broadcast, org summary,
    batch tracker. (Pre-M3 also held a compile-only store field for
    continuity across the M2.cd cutover; M3 deleted both the field
    and the underlying v1 evidence-store module, which had no
    production readers/writers post-M2.cd.) The read-model is served
    from `AppState::projection_state` via `lock_projection()`
    (`src/app/state.rs:311-317`).
  - `RunService`, `RepoService`, `WebhookService` — three
    ApplicationServices per CHE-0054:R4, wired on `AppState`
    (`src/app/state.rs:239-246`) and constructed in
    `build_services(...)`.
- **`projection_state` field (M2 cutover).** `AppState` carries
  `projection_state: Arc<Mutex<EvidenceProjection>>`
  (`src/app/state.rs:210`) initialised to
  `EvidenceProjection::default()` and populated by
  `app::projection_runtime::snapshot_fast_path_startup` after
  `with_stores` and before warm-start (CHE-0048:R2 — snapshot is the
  source of truth at boot). Read sites acquire the lock via
  `AppState::lock_projection()`; the bus handler is the sole writer
  via `Projection::apply` (CHE-0048:R2).
- **Cross-cutting fields** stay directly on `AppState`: run metadata,
  work queue, worker pool guard, event bus, the three domain-key
  indices (`run_index`, `repo_index`, `delivery_index`).
- **Persist-then-publish discipline (CHE-0024:R1 + BC-v2-1).**
  ApplicationServices do, in this order:
  1. construct `EventEnvelope`s for the new domain events;
  2. `event_store.append(envelopes, correlation).await?`;
  3. `bus.publish(&envelopes).await` via the `publish_or_trace`
     helper (`src/app/services/shared.rs:234-255`) — synchronous
     in-process delivery drives `EvidenceProjection::apply` via
     the registered bus handler; per CHE-0024:R1 publication
     failure is non-fatal because events are already durable; per
     CHE-0024:R3 + COM-0019:R1/R4/R7 a structured per-envelope
     `tracing::error!` emission carries `event_id`,
     `correlation_id`, `causation_id`, `aggregate_id`,
     `event` label, and `error` so tracking consumers can reconcile
     via checkpointed replay (§12).
  Reversal of (2) and (3) is forbidden.
- Work queue + worker pool come from `cherry-pit-runtime` (CHE-0052,
  v1 BC-1..BC-3).
- Rate limit + budget + pagination come from `cherry-pit-runtime`
  (v1 BC-4, BC-5).

Credential lifecycle unchanged: GitHub App tokens auto-refresh via
`ensure_credential()` on the long-lived client; PAT credential rotation
requires daemon restart.

## 5. Storage layout

**W1 truth-alignment note:** the layout below described the M2 cutover's
`cherry-pit-gateway` byte-adapter substrate. It is superseded for
`DomainEvent` durability by CHE-0073 + CHE-0074: `<store_dir>` (default
`./store/`) lays out one `.pgno` container per current-state class under
`events/`, plus the process fence (CHE-0043); there is no separate
projection-snapshot subtree — `EvidenceProjection` is not persisted to
disk at all. It is rebuilt in memory on every boot by folding
`NativeStore::events()` in line order (CHE-0073:R7); the sole writer to
`projection_state` is this event-fold rebuild
(`src/app/state.rs:404-406`), not a snapshot load.

```
<store_dir>/
  events/
    events.pgno                        # crate::store::NativeStore —
                                        # FiberStore<DomainEvent>, one
                                        # pardosa fiber per repository
                                        # domain key (CHE-0074:R4);
                                        # `.pgno` container (default
                                        # backend) or NATS JetStream
                                        # per CHE-0072 backend
                                        # selection.
    org-events.pgno                    # crate::store::NativeOrgStore —
                                        # FiberStore<OrgStateCaptured>,
                                        # one fiber per org identity
                                        # (CHE-0073:R8).
    team-events.pgno                   # crate::store::NativeTeamStore —
                                        # FiberStore<TeamStateCaptured>,
                                        # one fiber per (org, team_slug)
                                        # pair (CHE-0073:R10, CHE-0089).
    sweep-timeout-schedules/           # SchedulerEventStoreImpl —
                                        # cherry_pit_gateway::MsgpackFileStore,
                                        # non-persisted-by-CHE-0074 class
                                        # (`src/app/state.rs:48-51`).
    sweep-timeouts/                    # SweepTimeoutEventStoreImpl —
                                        # cherry_pit_gateway::MsgpackFileStore,
                                        # same non-persisted-by-CHE-0074
                                        # class.
  locks/
    <filename>.lock                   # CHE-0043 process-level fencing
                                       # (RunLock, BC-v2-18).
```

Removal of a repository, org, or team fiber is a soft delete via
pardosa's `detach` (CHE-0073:R2/R6); the pardosa envelope `detached` flag
is the sole soft-delete signal folded by the projection (CHE-0073:R7). A
returning identity is rescued via `rescue_detached`, not re-created.

All fiber-store writes go through pardosa's own atomic-append substrate;
`MsgpackFileStore` writes for the scheduler/sweep-timeout classes remain
atomic temp-then-rename per CHE-0032. The process-wide RunLock at
`<store_dir>/locks/...` fences the entire store (BC-v2-18).

**No `baseline/`, no `<YYYY-MM-DD>.checkpoint`, no projection-snapshot
files.** The v1 baseline file, the M2-cutover `FileProjectionStore`
snapshot/checkpoint pair, and any auto-migration code path are gone.
Rebuild is: replay the fiber(s) from the start, per CHE-0073:R7 fold
semantics — there is no separate snapshot artefact to delete.

## 6. Server (HTTP) shape

Inline-absorbs the upstream SERVE pipeline into
`crates/gh-report/src/infra/`:

| Symbol | New home |
|---|---|
| `ServerConfig`, `ServerConfigBuilder`, `ValidatedConfig`, `ConfigError` | `infra::server::config` |
| `ServerState`, `CachedPage`, `PageUpdateEvent`, `compute_etag`, `compress_zstd` | `infra::server::state` |
| `ServerError` (server-internal, 3-variant) | `infra::server::error` |
| `start`, `build_router`, middleware (security headers, WS handler, etc.) | `infra::server::server` |
| `wait_for_shutdown_signal` | `infra::signal` |
| `sanitize_path_segment` | `cherry_pit_web` (canonical; previously `infra::validate` — local copy deleted SM1 `sm1-sanitize-path-1779000001`) |

`sanitize_path_segment` is the highest-fanout symbol — re-imported by 6
collector modules (security_policy, dependabot, last_commit,
ghas_scanning, branch_protection, codeowners) from `cherry_pit_web` (flat
re-export per cherry-pit-web/src/lib.rs:68). Collectors update import
paths; behaviour unchanged.

Webhook handler + cached HTML pages remain at `src/webhook/` and
`src/server.rs` respectively, consuming the absorbed types via
`crate::infra::server::*`.

Public-API boundary: `crate::error::ServerError::Runtime(String)` is
the single opaque variant exposed via `AppError`. The server-internal
`infra::server::error::ServerError` (3-variant: `InvalidAddress`,
`BindFailed`, `RuntimeFailed`) is collapsed at the daemon boundary
(`app/daemon.rs`) via `e.to_string()`. This preserves donor variant
fidelity inside the server module while keeping gh-report's public
error surface stable.

Absorption status: complete. The upstream library dependency has been
dropped from `[dependencies]`.

## 7. From-scratch deploy

v2 ships from-scratch. There are no v1 deployments to migrate, and no
baseline-migration code path exists in the binary (locked posture U1).
A first daemon run against a fresh `<store_dir>` initialises empty
`events/`, `projections/`, and `locks/` subtrees on demand; the
projection's first snapshot lands after the first full collect cycle
completes and `EvidenceProjection::apply` has folded the resulting
envelopes. There is no operator action required between install and
first run beyond pointing `--store-dir` at a writable path.

## 8. Test strategy

- **Inline unit tests categorised at B11' per S6.H6.b.** The current
  ~692 unit tests split into three buckets relative to the v2 model:
  - **5 keep** — read-only view tests, exercise the projection's
    public read surface, untouched by the migration.
  - **6 rewrite** — direct-mutation tests that today poke
    `evidence_store` in-place; rewritten to publish the equivalent
    `DomainEvent` envelopes through the bus and assert via the
    projection's read surface.
  - **3 delete** — internal-state-of-AggregateStore tests; meaningless
    once `AggregateStore` is deleted at B9'.
  U5 hopper-tier refinement (final bucket assignment per file)
  permitted; **abort if the "delete" bucket inflates to ≥5 files**
  (charter §5.2).
- **One end-to-end smoke test** at `tests/smoke.rs` (B11'):
  - Spawns daemon against wiremock GitHub.
  - Runs one collect cycle.
  - Asserts `<store_dir>/events/<org>/1.msgpack` exists and is non-empty.
  - Asserts `<store_dir>/projections/<org>/1-evidence.snapshot.msgpack` exists.
  - Asserts HTML page served with expected status tier.
- Existing dev-deps cover this (wiremock, insta, proptest, tower,
  tokio-tungstenite, futures-util).
- **Mid-WU linus reviews** per B12': B7'+B8' typed seam closure;
  B9' AggregateStore deletion; B10' `App::run` conformance.
- Full verify suite plus `adr-fmt --lint` gates each
  sub-mission.

### Performance stance

No committed latency benchmark is maintained, and this is deliberate. The
performance posture is **memory-bounded-by-design**: working-set bounds are a
structural property of the projection model rather than a tuned target. Live
memory is observed via a runtime `rss_kb` gauge, and heap behaviour is
inspected on demand through a feature-gated `dhat` harness. There is no
serve-path latency SLO — the daemon's work is batch collection, not
low-latency request serving — so there is no latency contract to regression-
gate, and therefore no committed latency bench. Should a serve-path SLO ever
be introduced, a bench guarding it would follow; until then a bench would
assert against an invented target and add maintenance cost without protecting
a real contract.

## 9. README (DoD-4)

`crates/gh-report/README.md` is created in C-phase with 9 sections:

1. One-paragraph what-it-does (per §1 scope).
2. Install (currently git/path; `cargo install gh-report` future-tense).
3. Quick start (`--org` example).
4. CLI flag table (per §2).
5. Storage layout (cite DESIGN.md §5).
6. Security model (token tiers, `secrecy` crate behaviour, webhook HMAC).
7. From-scratch deploy (per §7).
8. License (MIT; matches Cargo.toml).
9. Links to DESIGN.md + OPERATIONS.md.

## 10. Implementation discretion

DESIGN.md prefers **idiomatic implementation per the binding ADRs over
strict prescriptive non-goals**. When CHE-0018 (sync domain / async
infra), CHE-0024 (event delivery), CHE-0029 (workspace graph),
CHE-0032 / CHE-0036 / CHE-0043 (atomic writes / file-per-stream /
process fencing), CHE-0048 (cherry-pit-projection), CHE-0050
(MsgpackFileStore), CHE-0051 (cherry-pit-app), CHE-0052
(cherry-pit-runtime), or CHE-0053 (cherry-pit-storage)
prescribe a shape, follow the ADR even where it deviates from the
default surgical-extraction posture. Implementation
discretion sits with hopper, bounded by:

- The ADRs above (binding).
- The 14 v1 BCs in oracle bead `adr-fmt-a6a` and the 19 v2 BCs in
  oracle bead `adr-fmt-...`.
- The DAG and abort criteria in `.ooda/mission-wu6v2-charter-1778415390.md`.
- This DESIGN.md (binding for §1–§12 shape calls).

Defaults that hold absent ADR override:

- No CLI subcommand redesign in v0.1.
- No removal of inline unit tests during rewrite beyond the 3
  AggregateStore-internal-state tests (per §8 bucketing).
- No behavioural change to collectors — wire-rewrite only.
- No `cargo publish` in WU-6 v2.
- No edits to `OPERATIONS.md` beyond the storage-layout section (C2').
- **Aggregate impls present, `CommandGateway` absent** (CHE-0054:R4
  + R10). `Run`, `Repo`, `WebhookDelivery` are full DDD aggregates
  with dedicated ApplicationServices that own the
  load → handle → append → publish triad directly against
  `cherry-pit-core::EventStore` + `EventBus`. gh-report does **not**
  implement `cherry-pit-core::CommandGateway` and does **not**
  consume `cherry-pit-app::App<…>` at v0.1 (CHE-0054:R8 + R10).
  Cross-aggregate reactions (e.g. `Run` → `Repo`) are dispatched at
  the call-site by ApplicationService methods invoking the downstream
  service directly, deferring `Policy::react`-driven choreography to
  a future ADR when a saga-class workflow appears.
- **Three-aggregate decomposition** per CHE-0054:R1+R2+R3 supersedes
  the v1 Tension-2 single-aggregate posture. `OrgGovernance` persists
  only as a documentary marker pinning the singleton snapshot id
  (§3); the *write* side is the three-aggregate decomposition.
- **No v1-migration code paths** (U1 lock).

## 11. Downstream-DAG implications surfaced by this design gate

Captured here so moltke can absorb them without re-running the interview:

1. **AggregateStore is deleted, not ported.** B9' deletes
   `evidence_store.rs` along with its `AggregateStore<K, V, M>` import.
   No A9 / cherry-pit-projection port sub-mission exists in v2; bead
   `adr-fmt-o09` is closed at A8' with reason "AggregateStore not
   ported per WU-6 v2 charter". The v1 §3a corrigendum routing it
   through cherry-pit-projection is **withdrawn**.
2. **App::run conformance is a B-phase requirement.** Wired at B10'
   (cherry-pit-app `App` composition: bus + EventStore + driver +
   projection store registered before `App::run` is called).
3. **DoD-4 expansion.** `OPERATIONS.md` gets a storage-layout section
   in C-phase (C2'), reflecting the v2 `events/` + `projections/`
   subtrees and the rebuild operational primitive (CHE-0048:R4).

## 12. EventStore + Projection contract

This section names the concrete cherry-pit consumers wired in §4 and
fixes their contract with gh-report code post-M2 cutover.

- **EventStore impl (W1 truth-alignment; CHE-0073 + CHE-0074).**
  `crate::store::NativeStore` — `pardosa_fiber_store::FiberStore<DomainEvent>`
  over `pardosa::store::EventStore<DomainEvent>` — one instance per
  process, backed by a `.pgno` container (default) or NATS JetStream
  (CHE-0072 backend selection), constructed at
  `<store_dir>/events/events.pgno`. One pardosa fiber per repository
  domain key (CHE-0074:R4); first observation of a key begins a fiber,
  subsequent observations append to it. On boot, `FiberIndex<domain_key>`
  is rebuilt from the log and `resume_defined` appends to an existing
  Defined fiber (CHE-0074:R5). This supersedes the retired
  `cherry_pit_gateway::MsgpackFileStore<DomainEvent>` /
  `PardosaEventStore` byte-adapter contract in its entirety — no
  `EventEnvelope`-as-bytes payload, no adapter-owned logical-stream
  reconstruction (CHE-0074:R8). Removal appends a `RepositoryStateCaptured`
  and detaches the fiber (`pardosa::StoreWriter::detach`); the envelope
  `detached` flag is the sole soft-delete signal, and a returning
  repository is rescued via `rescue_detached` (CHE-0073:R2/R6,
  CHE-0074:R6). Companion `crate::store::NativeOrgStore` /
  `crate::store::NativeTeamStore` provide the org-fiber (CHE-0073:R8) and
  per-team-fiber (CHE-0073:R10) current-state classes at
  `<store_dir>/events/org-events.pgno` and `…/team-events.pgno`.
  `cherry_pit_gateway::MsgpackFileStore` remains wired only for the
  non-persisted-by-CHE-0074 scheduler and sweep-timeout event classes
  (`SchedulerEventStoreImpl`, `SweepTimeoutEventStoreImpl`,
  `src/app/state.rs:48-51`); the `create` / `append` / `load` semantics
  described for those two classes (CHE-0013, CHE-0016, CHE-0019) are
  unaffected by this corrigendum.
- **ApplicationServices (CHE-0054:R4).** `RunService`, `RepoService`,
  `WebhookService` are the sole entry points to
  `EventStore::append` + `EventBus::publish` in production code paths
  (CHE-0054:R7 + R10). Each service owns its aggregate's
  load → handle → append → publish triad and threads caller-tracked
  CAS sequence numbers per CHE-0054:R6 + CHE-0042:R3.
- **Projection impl (production read-model).** `EvidenceProjection`
  implements `cherry_pit_core::Projection<Event = DomainEvent>`
  (`src/projection.rs`).
  - `apply(&mut self, envelope: &EventEnvelope<DomainEvent>)` is
    **synchronous** (CHE-0018:R1), **infallible** (CHE-0009), and
    **idempotent** over the same envelope sequence (CHE-0048:R3 +
    BC-v2-6).
  - **Sole writer and sole reader** of the gh-report read-model per
    CHE-0073:R7. Reads are served through the
    inherent API documented below; production read sites acquire
    the lock via `AppState::lock_projection`
    (`src/app/state.rs:407`).
  - **Inherent read API** (`src/projection.rs:138-180`):
    - `get(&self, key: &str) -> Option<RepositoryEvidence>` — per-repo
      lookup by `domain_key`.
    - `len(&self) -> usize` — repository count.
    - `sorted_snapshot(&self) -> Vec<RepositoryEvidence>` — clone of
      all entries sorted by `(repository.id, repository.name)`;
      required for BC-v2-6 snapshot byte-identity and HTML render
      stability.
  - **Inherent bulk-load API** (`src/projection.rs:201-220`,
    `bulk_load` private merge helper at `:222-229`).
    `load_baseline(&mut self, entries: Vec<RepositoryEvidence>)` and
    `load_resumed_checkpoint(&mut self, entries: Vec<RepositoryEvidence>)`
    are startup-only direct mutations, authorised by CHE-0048:R2's
    sole-writer posture *only* before `build_services` returns and
    before the bus is observable (M2 parent brief D2 + pre-mortem #7).
    Merge semantics: last-writer-wins on `inventory_key` collision
    via `BTreeMap::extend`; the two methods are body-identical at
    v0.1 — the distinction is documentary so saga warm-load
    (W4-then-W3) call-sites stay intent-visible.
- **ProjectionStore / Driver — W1 truth-alignment: withdrawn.**
  `cherry_pit_projection::FileProjectionStore<EvidenceProjection>` and
  `cherry_pit_projection::ProjectionDriver` are no longer wired; no
  occurrence remains in `crates/gh-report/src`. There is no persisted
  projection snapshot or checkpoint. `EvidenceProjection` is rebuilt
  from scratch on every process start by folding
  `NativeStore::events()` in line order — non-detached snapshots
  upsert, `detached == true` removes (CHE-0073:R7) — with the org and
  team folds applied independently and eventually consistent
  (CHE-0073:R9). The `load_baseline` / `load_resumed_checkpoint`
  bulk-load API above remains the mechanism for this startup fold; it
  is driven directly, not through a `ProjectionDriver`.
- **Bus (W1 truth-alignment).** `cherry_pit_app::InProcessEventBus<DomainEvent>`
  per CHE-0051:R2 remains wired for the non-persisted-by-CHE-0074 event
  classes (`crates/gh-report/src/app/collect.rs`). It no longer drives
  `EvidenceProjection::apply` — there is no `ProjectionDriverExt` /
  `apply_one` handler. ApplicationServices instead fold the durable
  write directly into the resident projection after the append lands
  (`AppState::fold_repository_event_into_projection`,
  `::fold_org_event_into_projection`, `src/app/state.rs:1398-1406`),
  runtime-fresh per CHE-0073:R7 detached-remove /
  non-detached-upsert — same fold rule as the boot-time rebuild, just
  applied incrementally instead of replayed from sequence 1.
- **Persist-then-publish ordering.** ApplicationServices call
  `event_store.append(envelopes, correlation)` first, then
  `publish_or_trace(&bus, &envelopes, event_label)`. No exception
  (CHE-0024:R1, BC-v2-1).
- **Bus-publish failure handling — `publish_or_trace`
  (CHE-0024:R1+R3 + COM-0019:R1/R4/R7).**
  `crates/gh-report/src/app/services/shared.rs:234-255` is the single
  absorb point. On `EventBus::publish` error the helper emits **one
  structured `tracing::error!` per envelope** under target
  `gh_report.eda` with fields:
  - `event_id` — `EventEnvelope::event_id()`
  - `correlation_id` — `EventEnvelope::correlation_id()` (COM-0019:R4
    correlation propagation across the observability boundary)
  - `causation_id` — `EventEnvelope::causation_id()`
  - `aggregate_id` — `EventEnvelope::aggregate_id()`
  - `event` — static event label (`"SweepStarted"`, `"RepoEvaluated"`,
    …)
  - `error` — `Debug` of the underlying `EventBus` error
  Fields are **structured `tracing` kv pairs**, never string-interpolated
  into the message (COM-0019:R1 + R4). `error!` severity is mandatory
  per COM-0019:R7 (EventBus retry-absorb telemetry — failures are
  operator-actionable). Per CHE-0024:R1 publication failure is
  non-fatal because events are already durable on the EventStore;
  per CHE-0024:R3 tracking consumers reconcile via checkpointed
  replay from `EventStore::load`. The persisted event sequence is
  the system's source of truth — `publish` is notification, not
  commit (CHE-0024).
- **Dead-letter sink (reserved for future use).**
  `cherry_pit_app::DeadLetterSink` (CHE-0051:R7) is **not wired**
  in gh-report at v0.1; CHE-0054:R10 reserves the surface for future
  use. The publish-or-trace pattern above is the v0.1 contract:
  bus-publish failures emit structured tracing and rely on
  EventStore-replay reconciliation (CHE-0024:R3). The
  `DeadLetterSink` integration becomes relevant when (a) policy
  outputs that fail bounded retry per CHE-0024:R5 are introduced,
  or (b) tracking consumers grow beyond a single in-process
  projection. Neither holds at v0.1.
- **Per-aggregate lock degeneration.** CHE-0048:R7's per-aggregate
  in-process lock is satisfied at v0.1 by per-aggregate
  ApplicationService method serialisation and by the singleton
  snapshot id (BC-v2-10). RunLock provides the process-fencing
  half (BC-v2-18).
- **Correlation.** `CorrelationContext` is an explicit parameter on
  `EventStore::{create, append}` per CHE-0039 + BC-v2-19.
  ApplicationServices that lack a meaningful correlation context use
  `CorrelationContext::root_for_collect_cycle()` (or equivalent) —
  not `CorrelationContext::none()` post-WU-6.
- **Rebuild (W1 truth-alignment).** There is no snapshot/checkpoint
  artefact to delete: `EvidenceProjection` is always rebuilt from
  scratch, every boot, by folding `NativeStore::events()` (and the
  companion org/team fibers) per CHE-0073:R7. Rebuild-on-corruption is
  therefore the ordinary startup path, not a distinct operator
  procedure; the prior `cherry-pit-projection::rebuild_file` reference
  is withdrawn along with the FileProjectionStore wiring it served.
