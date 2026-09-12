# `gh-report` Debugging & Operational Observability Guide

Comprehensive operational handbook for probing, monitoring, and diagnosing `gh-report` across its logging, metrics, and storage layers, including root-cause signatures and known blindspots.

---

## 1. System Topology & Data Flow

```text
  [GitHub REST API]
         │ (Rate limited: 5000 req/hr)
         ▼
  [gh-report Daemon on Cloud Run]
    ├── Ingestion & Sweep Pipeline (crates/gh-report/src/app/collect.rs)
    ├── Memory Projection (EvidenceProjection in AppState)
    ├── NATS Storage Adapter (Pardosa 0.5.5 standard envelope C4.19)
    └── Web Server (cherry-pit-web / Axum on port 8080)
         │
         ├──► NATS JetStream (tls://connect.nats.mattilsynet.io:4222)
         │       └── Account: 3FUb5hddItnLAqAuKV8gbnLdxsc
         │       └── Streams: gh-report-org_4d617474696c73796e6574-v20*
         │
         └──► HTTP / WebSocket Clients (via Google IAP)
                 ├── /api/v1/status (Telemetry JSON)
                 ├── /healthz & /readyz (Probes)
                 └── HTML Dashboard (owners, scorecards, drilldowns)
```

---

## 2. Probing the Log Layer

`gh-report` emits structured JSON lines to stdout (`GH_REPORT_LOG_FORMAT=json`), ingested by Google Cloud Logging into `jsonPayload`. Tracing follows Boyds/TigerStyle structured field discipline: typed metadata is attached as native JSON fields, avoiding string interpolation.

### Key Tracing Targets

| Module Target | Responsibility | Critical Fields |
|---|---|---|
| `gh_report::github::client` | GitHub API requests, rate limits, retries | `budget_calls`, `latency_ms`, `route`, `status`, `terminal_category`, `halt_until` |
| `gh_report::app::collect` | Sweep execution, partial report publishing | `batch_id`, `page_count`, `repo_count`, `timestamp` |
| `gh_report::app::state` | Store init, NATS connection, OCC resync | `endpoint`, `stream_stem`, `meta_subject`, `data_subject`, `server_id`, `replayed_events` |
| `gh_report::app::team_refresh` | Team roster synchronization | `team_domain_key`, `team_slug`, `missing_count`, `existing_count` |
| `cherry_pit_web::serve` | HTTP server, page cache, status codes | `method`, `path`, `status`, `latency_ms` |

---

### Cloud Logging Query Cookbook

#### A. NATS Connection & JetStream Provisioning
Inspect stream creation, envelope replay, and authentication errors:
```text
resource.type="cloud_run_revision"
resource.labels.service_name="ghreport"
jsonPayload.target="gh_report"
(jsonPayload.message=~"NATS" OR jsonPayload.boundary="nats_subjects")
```
* **Success signatures:**
  * `jsonPayload.message = "NATS connection established successfully"` (`server_id`, `server_version`, `proto`).
  * `jsonPayload.message = "NATS event stream created and claimed"` (first creation).
  * `jsonPayload.message = "NATS event stream replayed successfully"` (`replayed_events: N`).
* **Failure signatures:**
  * `jsonPayload.error =~ "permissions violation"`: Publish subject violates Synadia Control Plane authorization (`gh-report.>`).
  * `jsonPayload.error =~ "no artefact exists on open"`: Stream is missing or in an incomplete creation state.

#### B. GitHub API Quota, Throttling & Rate-Limit Halts
Monitor GitHub API consumption and detect automated sleeps:
```text
resource.type="cloud_run_revision"
resource.labels.service_name="ghreport"
jsonPayload.target="gh_report::github::client"
(jsonPayload.budget_calls > 0 OR jsonPayload.message=~"rate limit")
```
* **Key fields:**
  * `jsonPayload.budget_calls`: Current collection run request count (hard ceiling 4,000).
  * `jsonPayload.route`: Normalized endpoint template (e.g. `/repos/{owner}/{repo}/contents/{path}`).
* **Rate-Limit Halt signature:**
  ```text
  jsonPayload.message = "rate limit halt triggered — requests blocked until reset window"
  jsonPayload.halt_until = <unix_timestamp>
  ```
  The daemon pauses until the specified Unix timestamp. No requests are dispatched during this window.

#### C. Report Rendering & Cache Publication
Verify that HTML reports are updating every 15 seconds during collection:
```text
resource.type="cloud_run_revision"
resource.labels.service_name="ghreport"
jsonPayload.target="gh_report::app::collect"
jsonPayload.message="partial report published"
```
* **Key fields:**
  * `jsonPayload.page_count`: Number of rendered pages committed to cache (e.g. index, report, owners, detail pages, drilldowns).
  * `jsonPayload.batch_id`: Run UUID.

#### D. Team Roster Resolution & Fallbacks
Track team member fetching:
```text
resource.type="cloud_run_revision"
resource.labels.service_name="ghreport"
(jsonPayload.boundary="team_roster_missing_fetch" OR jsonPayload.boundary="team_roster_cold_start" OR jsonPayload.target="gh_report::app::team_refresh")
```
* **Success signature:**
  * `jsonPayload.boundary = "team_roster_missing_fetch"`: Unprojected CODEOWNERS teams fetched live and persisted.

#### E. Single-Writer OCC Fence Collisions
Observe revision rollovers when two Cloud Run instances overlap:
```text
resource.type="cloud_run_revision"
resource.labels.service_name="ghreport"
(jsonPayload.message=~"fenced by active single-writer guard" OR jsonPayload.terminal_category="fenced_conflict")
```
* **Normal Behavior:** During Cloud Run rolling updates, the retiring revision receives a fence error, aborts its sweep, and yields authority to the new revision.

---

# 3. Probing Metrics & Health Endpoints

`gh-report` provides HTTP endpoints for liveness, readiness, and runtime telemetry.

| Endpoint | Probe Type | Auth | Return Behavior |
|---|---|---|---|
| `/healthz` | Kubernetes Liveness | None / Anonymous | Always `200 {"status": "ok"}` |
| `/readyz` | Kubernetes Readiness | None / Anonymous | `200` if backend reachable AND (projection non-empty OR cached pages exist); `503` on cold boot |
| `/api/v1/status` | Runtime Telemetry | IAP / Bearer Token | `200` with JSON operational state |
| `/ws` | Real-time Broadcast | Origin checked | WebSocket stream broadcasting newly evaluated repository evidence |

### Probing `/api/v1/status` via `gcloud`
```bash
curl -s -H "Authorization: Bearer $(gcloud auth print-identity-token)" \
  "https://ghreport-nbd4qrr5oa-lz.a.run.app/api/v1/status" | jq .
```

#### Status Payload Fields:
```json
{
  "current_run": {
    "run_id": "01a09380-77cf-766a-91ba-9721d3fc055a",
    "timestamp": "2026-09-12T02:44:46.136954484Z",
    "auth_mode": "GitHubApp"
  },
  "last_completed_run": {
    "run_id": "01a09233-ef9e-726d-bfde-65c75e9ca573",
    "timestamp": "2026-09-11T20:43:29.049153038Z"
  },
  "last_recovery": null,
  "uptime_secs": 1845,
  "rss_kb": 128450,
  "projection_repo_count": 775,
  "projection_bytes_est": 1860000
}
```
* **`uptime_secs`**: Age of the container process.
* **`rss_kb`**: Process resident set memory (sampled from Linux `/proc/self/statm`).
* **`projection_repo_count`**: Number of active repository aggregates loaded in memory.
* **`current_run`**: Active collection metadata (`null` if idle).
* **`last_completed_run`**: Metadata of the last fully completed sweep.

---

# 4. Probing the Storage Layer (NATS JetStream & Pardosa 0.5.5)

In production, all state is durably backed by NATS JetStream at `tls://connect.nats.mattilsynet.io:4222` under account `3FUb5hddItnLAqAuKV8gbnLdxsc`.

### CLI Prerequisites
Retrieve credentials from Secret Manager or use the repo secret:
```bash
gcloud secrets versions access latest --secret nats-user-gh-report-creds \
  --project ghreport-d302 > /tmp/nats.creds && chmod 600 /tmp/nats.creds
```

### A. Listing Active Streams
```bash
nats --creds /tmp/nats.creds -s tls://connect.nats.mattilsynet.io:4222 stream list
```
* **Expected Active Streams for Major Version `v20`:**
  * `gh-report-org_4d617474696c73796e6574-v20_data`: Core domain log (repositories, organizations, teams).
  * `gh-report-org_4d617474696c73796e6574-v20_meta`: Epoch ownership claims and container headers.
  * `gh-report-org_4d617474696c73796e6574-v20-team-team_data` / `_meta`: Dedicated team roster events.
  * `gh-report-org_4d617474696c73796e6574-v20-org-org_data` / `_meta`: Organization alert snapshots.

### B. Probing Specific Events & Sequences
```bash
nats --creds /tmp/nats.creds -s tls://connect.nats.mattilsynet.io:4222 \
  stream get gh-report-org_4d617474696c73796e6574-v20_data <sequence_number>
```

#### Envelope Binary Layout (Pardosa 0.5.5 Standard Envelope `C4.19`)
* Bytes `0..16`: `fiber_id` (BLAKE3 hash of domain key).
* Bytes `16..32`: `event_id` (UUIDv7).
* Bytes `32..64`: `commitment` (rolling continuous BLAKE3 digest).
* Bytes `64..72`: `sequence` (`u64` monotonic sequence within fiber).
* Bytes `72`: `detached` (`0` = active, `1` = tombstone/deleted).
* Bytes `81..`: Event Discriminant Tag:
  * `0`: `RepositoryStateCaptured`
  * `1`: `RepositoryDeleted`
  * `2`: `OrgStateCaptured`
  * `3`: `TeamStateCaptured`

---

# 5. Known Operational Blindspots & Mitigation Workflows

### Blindspot 1: Secondary Rate-Limit 40-Minute Sleep
* **Mechanism:** When GitHub returns HTTP 403/429 with secondary rate-limit headers or quota drops below 100, `GitHubClient` calculates the sleep duration from `x-ratelimit-reset` and enters an asynchronous pause.
* **Blindspot:** Zero HTTP requests and zero collection logs are emitted during this window.
* **Diagnosis:** Inspect `/api/v1/status`. If `uptime_secs` advances while `current_run` remains active, the daemon is sleeping. Verify by searching logs for `rate limit halt triggered`. The web server remains healthy and serves existing reports.

### Blindspot 2: Cloud Run Ephemeral Container Discard
* **Mechanism:** Cloud Run instances are ephemeral. Container local disk (`/home/nonroot/store/`) is wiped when an instance stops or scales to zero.
* **Blindspot:** If `gh-report` is misconfigured to run on `PardosaBackend::Pgno`, writes succeed locally, but every cold boot starts with an empty projection.
* **Diagnosis:** Check startup logs for `boundary = "nats_fallback"`. If present, NATS is bypassed. Ensure `GH_REPORT_PARDOSA_BACKEND=nats` is set in Cloud Run.

### Blindspot 3: Out-of-Band Stream Deletion in Synadia Control Plane
* **Mechanism:** If an operator deletes a JetStream stream while `gh-report` is running, NATS does not push an eviction event to the client.
* **Blindspot:** The daemon serves from memory, but future appends fail or publish to an uninitialized stream without the container header (`PARDOSA\x01\x01`). On the next container restart, `open_read` fails with `no artefact exists on open`.
* **Remedy:** Cold-restart the Cloud Run revision so initialization can create and provision the stream headers.

### Blindspot 4: Permission-Denied Masking as "Unmeasured"
* **Mechanism:** Per COM-0028 doctrine (*"Error is not a negative finding"*), if the GitHub App token lacks permissions for private repository branch protection or secret scanning, the API returns `403 Forbidden`. The system maps this to `Unmeasured` (`ScoreCategory::Excluded`) rather than treating it as a security failure (`ScoreCategory::Fail`).
* **Blindspot:** If permissions rot organization-wide, the numerator and denominator drop equally. The displayed security percentage scores may remain at **100%** (100% of 0 observable controls).
* **Diagnosis:** Inspect the **Coverage** section and drilldown pages (`/security-policy-drilldown.html`, etc.) for high counts of `permission_denied`.

### Blindspot 5: CODEOWNERS Syntax Quirks vs Team Slugs
* **Mechanism:** CODEOWNERS files reference non-existent teams or user handles:
  * `@Mattilsynet/valid-team` $\to$ `OwnerType::Team` (resolves via team API).
  * `@Mattilsynet/*` $\to$ `OwnerType::AmbiguousTeamShaped` (`is_wildcard_owner = true`).
  * `@individual-user` $\to$ `OwnerType::User` (no team roster applies).
  * Deleted teams $\to$ Team fetch returns 404, persisted with `status: TeamRosterStatus::Deleted`.
* **Blindspot:** If a team is renamed on GitHub but CODEOWNERS still references the old slug, the old slug is marked `Deleted`. It appears on the **Deleted/Ghost Teams** page (`/deleted.html`), and its orphan repos cannot be attributed to the renamed team until CODEOWNERS is updated.

### Blindspot 6: Ephemeral Sweep Timers & Memory Queues
* **Mechanism:** Per CHE-0099, the internal queue (`cherry-pit-wq`) and sweep timeout handlers are strictly in-process and memory-resident.
* **Blindspot:** If Cloud Run recycles or restarts an instance mid-sweep, in-flight jobs leave no checkpoint in NATS. The replacement container boots and begins a fresh sweep from repository 1. The prior partial sweep leaves no residual corruption because Pardosa writes per-repository fibers atomically.

---

# 6. Actionable Troubleshooting Runbook

| Symptom | Root Cause | Immediate Action |
|---|---|---|
| HTTP 503 `content not yet available` | First boot before first 15-second partial render completes. | Wait 15 seconds; verify partial publisher logs (`page_count > 0`). |
| Container fails startup probe / crashes on boot | NATS credentials missing or subject namespace permission violation. | Check Cloud Run logs for `permissions violation` or `NATS connection failed`. Verify secret mount `/etc/nats/creds/user.creds`. |
| `<summary>Team Members (unresolved)</summary>` | CODEOWNERS references a team not yet fetched or deleted on GitHub. | Check `/deleted.html`. Verify whether `GET /orgs/{org}/teams/{slug}` returns 404 on GitHub. |
| Stream message counts stop advancing | GitHub API rate-limit halt (remaining < 100). | Check `/api/v1/status` and search logs for `rate limit halt triggered`. Do not bounce the service; it will resume automatically on the hour. |
| `fenced by active single-writer guard` in logs | Cloud Run revision rollover OCC conflict. | Expected operational event. Superseded revision yields to new revision. No intervention required. |
