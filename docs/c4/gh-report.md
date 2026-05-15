# C4 — gh-report

`gh-report` is a **GitHub organization governance collector and
reporter** (`crates/gh-report/Cargo.toml:6`). It runs as a long-lived
daemon behind Cloud Run / a reverse proxy, ingests data from the GitHub
API (REST + webhooks), event-sources a single organization-wide
aggregate via the cherry-pit family, projects evidence into an in-memory
read model, and serves HTML reports plus a JSON
`/api/v1/status` endpoint (`src/server.rs:34–41`).

Posture commitments worth knowing before reading:

- **Single-aggregate**: `ORG_GOVERNANCE_AGGREGATE_ID = NonZeroU64::new(1)`
  per Tension-2 single-aggregate lock (Cargo.toml:36–41).
- **Bus-only composition**: `cherry-pit-agent` is used for the
  `InProcessEventBus` + `ProjectionDriverExt`, **not** for the full
  `App`/`CommandGateway` wiring (Cargo.toml:42–50).
- **Durable event store**: `MsgpackFileStore<DomainEvent>` rooted at
  `<store_dir>/events/<org>/`.
- **Durable projection store**: `FileProjectionStore<EvidenceProjection>`
  rooted at `<store_dir>/projections/<org>/`,
  `projection_name = "evidence"`.

---

## L1 — System Context

```mermaid
C4Context
    title gh-report — System Context

    Person(operator, "Operator / SRE", "Reads HTML governance reports and consumes /api/v1/status JSON")
    Person_Ext(orgadmin, "GitHub Org Admin", "Configures the GitHub App installation that grants gh-report read access")

    System(ghreport, "gh-report", "Long-lived daemon that collects GitHub governance evidence, event-sources it, projects it, and serves it")

    System_Ext(githubapi, "GitHub REST/GraphQL API", "Source of branch protection, CODEOWNERS, dependabot, GHAS, inventory, commit metadata")
    System_Ext(githubwebhooks, "GitHub Webhooks", "Push-driven org event stream (HMAC-SHA256 signed)")
    System_Ext(ingress, "Cloud Run / Reverse Proxy", "Terminates TLS, enforces ingress authentication, forwards to gh-report")
    System_Ext(fs, "Local Filesystem", "Persists event store + projection snapshots/checkpoints")
    System_Ext(gcplog, "GCP Cloud Logging", "Receives structured JSON logs via tracing infra::cloud_logging layer")

    Rel(operator, ingress, "Reads HTML report / GET /api/v1/status", "HTTPS")
    Rel(orgadmin, githubapi, "Installs GitHub App granting gh-report read scope")

    Rel(ghreport, githubapi, "Polls (REST) — auth via GitHub App JWT", "HTTPS")
    Rel(githubwebhooks, ghreport, "POSTs signed events", "HTTPS")
    Rel(ingress, ghreport, "Proxies authenticated requests", "HTTP")

    Rel(ghreport, fs, "Reads/writes events + projections (msgpack)")
    Rel(ghreport, gcplog, "Emits structured JSON logs")
```

---

## L2 — Container

Internal containers correspond to the top-level modules of
`crates/gh-report/src/` (verified present). External cherry-pit crates
are aggregated into a single boundary box to keep edges legible — their
internals are detailed in `docs/c4/cherry.md`.

```mermaid
C4Container
    title gh-report — Containers

    Person(operator, "Operator / SRE", "")
    System_Ext(githubapi, "GitHub API", "")
    System_Ext(githubwh, "GitHub Webhooks", "")
    System_Ext(ingress, "Cloud Run / Reverse Proxy", "")
    System_Ext(fs, "Filesystem", "")

    System_Boundary(gh, "gh-report") {
        Container(cli, "bin/gh-report", "clap-derive CLI", "Process entry; selects command (daemon, collect, …)")
        Container(server, "server", "axum Router", "Builds the axum router (vendored SERVE layer: config, infra signal/validate, state); adds /api/v1/status")
        Container(app, "app", "composition root", "state, daemon, collect, evidence_service, projection_runtime, services{repo,run,webhook}, work_queue, worker_pool, github_infra, webhook_context")
        Container(collector, "collector", "GitHub data ingest", "branch_protection, codeowners(_parser), dependabot, ghas_scanning, inventory, last_commit, ref_matching, security_policy")
        Container(github, "github", "HTTP client layer", "auth (GitHub App JWT), client, dto, pagination, rate_limit, budget")
        Container(webhook, "webhook", "webhook handler", "HMAC-SHA256 signature verify; event dispatch")
        Container(aggregate, "aggregate", "single org aggregate", "ORG_GOVERNANCE_AGGREGATE_ID = 1; metrics")
        Container(domain, "domain", "domain model", "events, evidence, repository, run, codeowners, status, time, cache, checks, auth, metrics; aggregates/{repo,run,webhook}")
        Container(projection, "projection", "evidence projection", "Folds DomainEvent into EvidenceProjection read model")
        Container(report, "report", "HTML renderer", "askama templates + view_model")
        Container(config, "config", "configuration", "dashboard, runtime")
        Container(infra, "infra", "infra glue", "baseline, checkpoint, cloud_logging, lock (RunLock), logging")
    }

    System_Ext(cherrypit, "cherry-pit family", "core, gateway (MsgpackFileStore), projection (FileProjectionStore), agent (InProcessEventBus, ProjectionDriverExt), runtime (worker_pool, work_queue, rate_limit, pagination, budget), storage-primitives (atomic_write_bytes, RunLock, build_snapshot_signature)")

    Rel(operator, ingress, "HTTPS")
    Rel(ingress, server, "HTTP")
    Rel(githubwh, webhook, "POST signed event")
    Rel(cli, app, "Invokes daemon/collect")

    Rel(server, app, "Reads AppState (status payload, evidence)")
    Rel(server, report, "Renders HTML via")

    Rel(app, collector, "Drives periodic collection")
    Rel(app, webhook, "Dispatches inbound events")
    Rel(app, aggregate, "Issues commands against")
    Rel(app, projection, "Drives via ProjectionDriverExt")
    Rel(app, infra, "Acquires RunLock; checkpoints")
    Rel(app, config, "Loads")

    Rel(collector, github, "Fetches via")
    Rel(github, githubapi, "HTTPS (reqwest)")

    Rel(aggregate, domain, "Emits events from")
    Rel(projection, domain, "Folds events from")
    Rel(report, projection, "Reads view model from")

    Rel(aggregate, cherrypit, "core traits + gateway MsgpackFileStore")
    Rel(projection, cherrypit, "projection FileProjectionStore + agent ProjectionDriverExt")
    Rel(app, cherrypit, "agent InProcessEventBus + runtime primitives")
    Rel(infra, cherrypit, "storage-primitives atomic_write_bytes + RunLock")

    Rel(infra, fs, "Atomic file writes + RunLock")
    Rel(cherrypit, fs, "Event streams + projection snapshots")
```

---

## L3 — Component

Scoped to the two crate modules with the largest internal surface:
`app/` (the composition root) and `collector/` (the GitHub-evidence
ingest). `domain/`, `github/`, `webhook/`, `report/`, and `infra/` are
documented sufficiently by their container descriptions above; their
file lists are stable and small.

### app/ components

One component per `crates/gh-report/src/app/*.rs` (and the
`services/` sub-module split out separately).

```mermaid
C4Component
    title gh-report::app — Components

    Container_Boundary(app, "app") {
        Component(state, "state", "AppState", "Holds Arc<dyn ...> handles to event store, projection store, evidence cache; implements the vendored ServerState trait")
        Component(daemon, "daemon", "long-lived loop", "Orchestrates collect → emit events → wait → repeat")
        Component(collect, "collect", "one-shot collect", "Drives a single collection sweep against the GitHub API")
        Component(evidence_service, "evidence_service", "evidence read API", "Serves projected evidence to HTTP handlers")
        Component(github_infra, "github_infra", "github client wiring", "Builds the github::Client with auth/rate-limit/budget")
        Component(projection_runtime, "projection_runtime", "projection driver host", "Boots snapshot-fast-path, then drives ProjectionDriverExt::apply_one from the event bus")
        Component(webhook_context, "webhook_context", "webhook dispatch context", "Threads correlation + auth into webhook handlers")
        Component(work_queue, "work_queue", "queue facade", "Re-exports/specialises cherry-pit-runtime work_queue for gh-report jobs")
        Component(worker_pool, "worker_pool", "pool facade", "Re-exports/specialises cherry-pit-runtime worker_pool")

        Container_Boundary(services, "services") {
            Component(repo_service, "repo_service", "service", "Repository-level orchestration")
            Component(run_service, "run_service", "service", "RunMetadata + lifecycle")
            Component(webhook_service, "webhook_service", "service", "Webhook event ingest → aggregate command")
            Component(shared, "shared", "service helpers", "Cross-service helpers")
        }
    }

    Rel(daemon, collect, "Invokes per tick")
    Rel(daemon, state, "Reads/updates")
    Rel(daemon, projection_runtime, "Boots and drives")
    Rel(daemon, work_queue, "Schedules work via")
    Rel(work_queue, worker_pool, "Backed by")

    Rel(collect, github_infra, "Uses client built by")
    Rel(collect, repo_service, "Delegates per-repo work to")
    Rel(repo_service, run_service, "Reports lifecycle to")
    Rel(repo_service, shared, "Uses helpers from")

    Rel(webhook_service, webhook_context, "Reads from")
    Rel(webhook_service, state, "Updates aggregate via")

    Rel(evidence_service, state, "Reads from")
    Rel(evidence_service, projection_runtime, "Subscribes to projection updates")
```

### collector/ components

One component per `crates/gh-report/src/collector/*.rs`.

```mermaid
C4Component
    title gh-report::collector — Components

    Container_Boundary(collector, "collector") {
        Component(inventory, "inventory", "module", "Enumerates org repositories")
        Component(branch_protection, "branch_protection", "module", "Branch protection rules per repo")
        Component(codeowners, "codeowners", "module", "CODEOWNERS file retrieval")
        Component(codeowners_parser, "codeowners_parser", "module", "CODEOWNERS syntax parser")
        Component(ref_matching, "ref_matching", "module", "Ref/path matching against CODEOWNERS rules")
        Component(dependabot, "dependabot", "module", "Dependabot config + alerts")
        Component(ghas_scanning, "ghas_scanning", "module", "GHAS secret/code scanning state")
        Component(security_policy, "security_policy", "module", "SECURITY.md presence + content")
        Component(last_commit, "last_commit", "module", "Most recent commit per default branch")
    }

    System_Ext(github_client, "github::client", "Authenticated HTTP client")

    Rel(inventory, github_client, "Lists repos via")
    Rel(branch_protection, github_client, "Fetches rules via")
    Rel(codeowners, github_client, "Fetches file via")
    Rel(codeowners, codeowners_parser, "Parsed by")
    Rel(codeowners_parser, ref_matching, "Produces rules consumed by")
    Rel(dependabot, github_client, "Fetches alerts via")
    Rel(ghas_scanning, github_client, "Fetches state via")
    Rel(security_policy, github_client, "Fetches SECURITY.md via")
    Rel(last_commit, github_client, "Fetches HEAD commit via")
```

---

## Notes & non-goals

- The diagrams describe modules **physically present** in
  `crates/gh-report/src/` at the time of writing. New top-level
  modules will require updates here.
- The `cherry-pit family` external box on the L2 diagram is the same
  set of crates detailed in `docs/c4/cherry.md`; cross-reference
  there for cherry-pit internals.
- The vendored SERVE layer (`config`, `infra/{signal,validate}`, `state`,
  `server`, `run`) is now internal to `gh-report` after the Phase-1 P1-A
  donor-crate absorption (formerly a separate workspace crate, removed
  per bead `adr-fmt-6hmi`).
- No L4 (code-level) diagrams.
