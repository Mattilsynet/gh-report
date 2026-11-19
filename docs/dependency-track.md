# Dependency-Track: released inventory and VEX triage

The release workflow [build.yml](../.github/workflows/build.yml) publishes one
CycloneDX 1.5 BOM per release tag. The upload always addresses the same
Dependency-Track project identity:

| field | value |
|---|---|
| API | `https://api.dtrack.mattilsynet.io` (not the browser UI host) |
| `projectName` | `gh-report` |
| `projectVersion` | `prod` |

Those two strings are literals in both workflows (RST-0008:R1). With
`auto-create: true`, any other value would silently create a new, untriaged
project, so they are never taken from a tag, SHA or dispatch input.

## What the BOM describes

- **Root**: the released image, pinned to the digest that the build job pushed,
  as `pkg:oci/gh-report@sha256%3A<hex>?repository_url=…&tag=…`.
- **Components**: the shipped cargo closure of `gh-report` for
  `x86_64-unknown-linux-gnu`, plus whatever the image scan yields.
- **Compile-time-only crates are excluded**: the closure follows normal
  dependency edges and stops at every proc-macro package, so proc-macros and
  the crates only they reach are not presented as shipped inventory. The
  merge step names every excluded crate and reason in its log
  (`excluded: <crate> (proc-macro|behind-proc-macro|not-in-shipset)`), and
  `--check` rejects `basic-toml`, `paste`, `proptest`, `insta` and `dhat`
  outright.
- **OS packages**: the runtime base image is
  `gcr.io/distroless/cc-debian12:nonroot`, which ships no package database.
  The image scan therefore contributes the image identity and approximately
  zero OS components. This is not OS coverage; see RST-0008.
- **Not inventoried**: the committed Wasm client bundle that the server embeds
  with `include_bytes!`. It is prebuilt bytes, not part of the server's cargo
  graph. Tracked as a follow-up.

The upload is performed by the job that deploys to Cloud Run, after the
service reports `Ready`, so the project reflects what is confirmed running.
Consequences of that ordering: a failed upload marks an otherwise-successful
release run red (the service is running, but its inventory was not delivered),
and the upload is accepted for **asynchronous** processing, so a green release
run means "accepted", not "analysis complete".

`DTRACK_API_KEY` must be a repository secret, or additionally exposed to the
`deploy` job: that job deliberately does not use a protected environment, and
an environment-scoped secret alone would fail the upload while the VEX job
(which runs in `prod`) succeeds. The key needs `BOM_UPLOAD`,
`PROJECT_CREATION_UPLOAD` for `auto-create`, and `VULNERABILITY_ANALYSIS` (or
`VULNERABILITY_ANALYSIS_UPDATE`) for the VEX endpoint.

## Applying VEX triage

Triage decisions are kept in
[`crates/gh-report/dtrack-vex.json`](../crates/gh-report/dtrack-vex.json).
The file is the source of truth; applying it overwrites the analysis state of
each matched finding, including decisions made by hand in the UI.

Run **Actions → Apply VEX to Dependency-Track → Run workflow** from `main`.
The job is gated to `main`, runs in the `prod` environment (configure it with
Required reviewers if triage needs approval), and posts the file to
`POST /api/v1/vex` as multipart with the literal `projectName`, `projectVersion`
and `vex=@crates/gh-report/dtrack-vex.json`.

The workflow prints the number of decisions in the file before the POST and
always proceeds, so an empty file still reaches the endpoint and the
`404`-before / `200`-after ladder stays observable. Dependency-Track 5.1.2
accepts an empty `vulnerabilities` array: the POST returns `200` with a
`BomUploadResponse` token, the importer logs one INFO line
(`The uploaded VEX does not contain any vulnerabilities; Skipping VEX import`)
and applies nothing. On that path the workflow emits exactly one notice saying
the file contains 0 decisions, that the document was accepted and applied
nothing, and that the run proves the dispatch path, not suppression. A `200` is
never reported as triage applied, and a `200` without the token is a separate
warning.

### What the HTTP status means

| Status | Meaning | Action |
|---|---|---|
| `200` | Accepted for asynchronous processing. Entries that match an existing finding are queued; the audit trail later shows the commenter `CycloneDX VEX`. | Verify the analysis state in the project UI. A 200 does not mean an entry matched. |
| `403` | `DTRACK_API_KEY` lacks `VULNERABILITY_ANALYSIS` (or `VULNERABILITY_ANALYSIS_UPDATE`) or access to `gh-report / prod`. | Fix the key scope. Never print the key. |
| `404` | The project `gh-report / prod` does not exist yet: no SBOM has been imported under those literals, or the mapping drifted from the upload job. | Run a release (or fix the literals), wait for the import, then retry. |
| other | Transport or server error. | Treat as a delivery failure; the job exits non-zero. |

The endpoint cannot create projects. A `404` is therefore also the check that
the VEX workflow and the upload job address the same identity.

### What the API server log can and cannot show

Per-entry skips are not visible at the deployed log level. The API server runs
the `org.dependencytrack` logger at `INFO`, while the informative per-entry
skips — a VEX entry without `affects`, or without `analysis` and no OWASP
rating — are logged at `DEBUG`. At `INFO` only the whole-document
`Skipping VEX import` variants and the "unable to locate"/"not affected" WARN
paths speak. A future investigation that needs the per-entry lines must ask
the platform owner to raise
`org.dependencytrack.parser.cyclonedx.CycloneDXVexImporter` to `DEBUG` for the
window. The 5.1.2 image honours the `dt.logging.level."<logger>"`
configuration key at startup; the environment-variable spelling of that key is
not source-verified at 5.1.2, so state the key form and let the platform owner
map it. No repository change here can substitute for that.

The packaged logback encoder is plain text and prints the *simple* logger name
(`%logger{0}`), so a Cloud Logging filter on the fully qualified class name
matches nothing. Use:

```text
resource.type="cloud_run_revision"
resource.labels.service_name="dtrack-apiserver"
textPayload:"CycloneDXVexImporter"
```

and, to see only the skip lines, `textPayload:"Skipping VEX import"`.

With the zero-decision file, the apiserver logs, in order: `VEX upload accepted`
(the controller), `Starting VEX import`, `Processing CycloneDX VEX uploaded.`,
the single INFO skip line `The uploaded VEX does not contain any
vulnerabilities; Skipping VEX import`, `Completed processing of CycloneDX VEX`,
and `VEX import completed`. `VEX import completed` and `Completed processing of
CycloneDX VEX` are distinct events, not the same line. No count of applied
analyses exists anywhere on this path.

Two artefacts do **not** mean triage was applied: an empty document still
persists a `Vex` row and still emits `VEX_CONSUMED` and `VEX_PROCESSED`
notifications, because the importer returns before any analysis is written.
Read a Vex row or those notifications as "the document was processed", never as
"the decisions were applied".

Reading the per-entry outcome requires access to the API server's log stream,
so it cannot be observed from this repository or from the Dependency-Track UI.
Without that access the honest end state is: the dispatch returned `200`, and
the analysis state in the UI is the only evidence of an applied decision.

## First live proof: release `v0.1.108`

The proof ladder has three rungs, in this order:

1. **Upload and import.** Tag `v0.1.108` (a human does this; agents never
   create, move or push tags). The `Build` workflow's `build` job pushes the
   image and generates the BOM, then `deploy` updates Cloud Run, verifies
   `Ready`, and uploads. In Dependency-Track, open project `gh-report`, version
   `prod`: the import completes asynchronously, so check that components appear
   and that the root component is the OCI image at the pushed digest. This
   proves the project exists under the literals.
2. **VEX dispatch.** Dispatch **Apply VEX to Dependency-Track** from `main`.
   Before the first successful SBOM import is ready, expect `404`; after it,
   expect `200`. The literals are identical to the upload by construction, so a
   `404` means the import has not landed, not that the mapping is different.
3. **Per-entry application.** `200` plus the skip-log line is all that is
   visible without API-server-log access, and this rung is therefore unproven
   for `v0.1.108`. To close it, ask the platform owner for the Cloud Logging
   filter above at `INFO` for the dispatch window, and if the per-entry lines
   are needed, for `CycloneDXVexImporter` raised to `DEBUG`. A persisted `Vex`
   row and `VEX_CONSUMED`/`VEX_PROCESSED` notifications are not evidence for
   this rung.

> Caveat for this release: the file currently carries no decisions. Both
> `deny.toml` advisories (`RUSTSEC-2024-0436`, `RUSTSEC-2026-0374`) are
> compile-time-only and are excluded from the released inventory by
> RST-0008:R3, so seeding them would create entries that can never match
> (RST-0008:R6). `v0.1.108` therefore proves the dispatch mechanism
> (`404` → `200`) and the project identity, not suppression, and the
> `CycloneDX VEX` commenter cannot appear in the audit trail while the file has
> no decisions. The commenter becomes observable the first time a decision is
> added for a shipped component that has a finding.

## Adding a decision

Add the entry to `crates/gh-report/dtrack-vex.json` in the change that fixes or
justifies the finding, run `python3 -B tools/test_dtrack_vex.py`, and let the PR
review approve the decision. Match the `id` and `source.name` exactly as the
finding shows them in Dependency-Track, and describe the affected component
with the same purl as the SBOM. Every `affects[].ref` must resolve to a
component `bom-ref` in the release inventory or to the metadata component; the
test rejects dangling refs. The format and matching rules are documented in the
reference guide: `dependencytrack-operations/docs/vex-triage.md`.

`deny.toml` and this file must agree: an advisory ignored for a compile-time
crate stays in `deny.toml` and is not seeded here; an advisory affecting a
shipped crate gets a VEX entry and, if the audit gate would otherwise fail,
its own SEC-0013-governed ignore with an exit trigger.
