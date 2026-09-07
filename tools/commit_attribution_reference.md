# Next scheduled schema cut: last-commit attribution preparation

Scope: ghr-ibiyo under ghr-p3jrc.32.5, 2026-09-06. Source baseline HEAD
`4279ab2514d3c5ff0717936e9f09b18fa21f220f`, with existing uncommitted changes.
This is an executable proposal, not a production policy, schema edit, rollout,
formal OCC model, or authorization for an attribution-only cut. Ride the next
**scheduled** major cut; the schedule is already decided. No new dependency.

## Original versus current source

Original evidence: ghr-ibiyo records the abandoned optional-author-field probe
and the explicit defer decision. Its old CHE-0082 amendment request is resolved:
CHE-0082:R2 (docs/adr/cherry/CHE-0082-gh-report-collection-health-taxonomy.md:23)
already states field appends move native hashes and prohibit mixed replay.

Current paths (all crate paths below relative to `crates/gh-report/src/`):

| Seam | Current source | Scheduled delta |
|---|---|---|
| Fetch | `collector/last_commit.rs:43-94` | Same single default-branch head response; capture both top-level `committer` and `author` user objects, their login and account type, plus separately labelled commit metadata. No additional history request. |
| DTO | `domain/evidence.rs:68-77` | Separate raw actor observations from validated display attribution. Existing fields alone cannot reconstruct author or account type. |
| Native payload | `event/mod.rs:166-178` | Bounded raw actor observation for both roles; explicit missing/unsupported type states, nonempty validated login boundary, no derived roster/team attribution stored. Final field/enum layout belongs to the scheduled contract. |
| Conversion | `event/convert.rs:456-484` | Both directions must preserve Unknown and role provenance; test missing/null, invalid strings, string capacity boundaries, and roundtrip. Conversion errors must not become negative identity conclusions. |
| Envelope | `event/mod.rs:604-621` | `DomainEvent::RepositoryStateCaptured.evidence` transitively contains changed `LastCommitInfo`. All variants share the enum hash, including repository deletion. |
| Display | `report/html.rs:1535-1593,1622` | Current name-before-login rendering shared with owner detail; keep name as unverified metadata, not account identity. Add explicit selected role. Do not silently label an author as committer. |
| Orphan join | `report/html.rs:1715-1751,1758-1803` | Predicate unchanged. Current join uses raw committer login independently of display. Feed selected linked login to both display and roster join, never a name/email. Team membership is a suggestion, not CODEOWNER proof; multiple roster matches require explicit ambiguity rather than an ownership assertion. |
| Version/identities | `config/mod.rs:35,41`; `OPERATIONS.md:472-515` | Pair evidence version and major token, append history; verify repo/org/team stream, subject, consumer identities and file backend paths separately. |

## Proposed legal attribution states and precedence

The reference models `Unknown(reason) | System(reason) | LinkedUser(login)`;
selection is `Selected(role, LinkedUser) | Unknown(reason)`. Python dataclasses
are not sealed Rust types: direct construction is possible. A future Rust
implementation needs private validated constructors and a role enum, including
all serde/native construction and mutation paths. No compile-time proof here.

`LinkedUser` means a GitHub response linked this commit role to an account of
type User; it does **not** prove a natural person, verified authorship, employment,
ownership, or responsibility. A User account can still be operated by automation.

| Raw observation | Candidate | Selection consequence |
|---|---|---|
| Recognized `type: Bot` | System | Skip even if login missing |
| `web-flow` (case-insensitive), `[bot]` login suffix | System | Skip; display name is irrelevant |
| Usable login AND `type: User`, no system signal | LinkedUser | Eligible |
| Missing/null user, missing login/type, unsupported type, malformed login | Unknown | Skip without claiming deleted, unregistered, human or bot |
| Only `commit.author/committer.name` or email | Unknown | Never synthesize a GitHub login, link, or team |
| Human-linked account display name `GitHub` | LinkedUser | Not excluded by name |
| Human-linked account with noreply email | LinkedUser | Privacy is not a bot signal |

Precedence is **committer, then author**, first eligible LinkedUser. A System or
Unknown committer permits an eligible author; neither eligible gives Unknown,
not an invented attribution. Preserve each raw reason separately in future
evidence even though the reference's final Unknown is intentionally coarse.
Names/email on a git commit are supplied metadata, not identity attestation.
Do not infer an account from `123+login@users.noreply.github.com`, or classify
every noreply address as a bot. The old bead's broad email alternative is not a
safe identity oracle. Null could reflect privacy, missing linkage, or other
unknown causes; this local fixture analysis does not establish GitHub's reason.
No live API or external specification was consulted in this preparation.

Minimum persisted input for this proposal: per-role observed login and account
type with Unknown representable. Existing committer name/date may stay; author
name is optional display metadata. Raw email retention is **not required** by
the reference: exclude it by default rather than expand PII collection. Any
future verified-email classification needs separate provenance/privacy review.
The reference login grammar is deliberately conservative, not a full GitHub
validator: unsupported spellings become Unknown, never a forged URL.

## Native compatibility matrix (not executed hash measurements)

Hash construction: `crates/pardosa-derive/src/hash.rs:14-49,63-89` folds type name,
field names, order and nested type hashes; enum variants/discriminants also fold.

| Change/case | Expected native compatibility |
|---|---|
| These standalone files; report-only logic, DTO-only addition not persisted | Native hash unchanged; DTO-only capture would be lost on replay |
| Optional native author field, even when every value is None | Hash changes; None is a value, not an omitted schema field |
| Native field rename/type/order, bound change, nested enum variant | Treat as schema-breaking; measure changed and enclosing hashes |
| New binary pointed at existing v19 repository stream/file | Must refuse incompatible schema, not reinterpret bytes |
| New binary plus fresh scheduled-major identities | Isolated cold start and re-scrape; no historical author backfill |
| Old binary pointed at new-major data | Unsupported; never assume reverse compatibility |
| Org/team native types unchanged but global major bumped | Measure their hashes independently; identities still change, so plan their cold-start reprovision too |

Historical probe in ghr-ibiyo (not rerun here): LastCommitInfo
`79e413fc75a36d3e998ab68589800f17` -> `9df586f8a7af4fc8de87bcced2fdd673`;
RepositoryEvidence `cbf9962a3a54c754d79bd509bc06fdce` ->
`b070a78e4375ce07e82df3b1e771b334`. These are not promises for the proposed
layout. Measure LastCommitInfo, RepositoryEvidence, DomainEvent, plus independent
OrgStateCaptured and TeamStateCaptured and store/container registration at cut.
CHE-0022:R3 requires refusal/re-scrape; serde defaults cannot bypass native hashes.

## Rollout preflight and rollback requirements — no migration run

1. At the already scheduled cut, capture exact old/new binary digests, native
   hash inventory and actual effective backend identities/config. Validate both
   version constants together. Snapshot/read-only inventory retained stores.
2. Exercise new-major fresh provisioning and same-major warm replay; prove
   old/new mismatch refuses. Cover nested raw actor states, conversions,
   collector response fixtures, owner/orphan HTML, links/escaping and team joins.
3. Reserve re-scrape API/time capacity and cold-cache readiness; no invented
   latency allowance. Verify all three stores/projections and report completeness
   before routing traffic. Baseline reuse must not invent absent author facts.
4. Preserve old streams unchanged; no mixed replay, purge, in-place rewrite,
   or automatic author backfill. `convert.rs` is DTO/native conversion, **not**
   an upcaster for v19 bytes. Offline migration needs a separate contract.
5. If schema gate, startup/readiness or attribution checks fail: stop new writes,
   keep both generations intact, and use the preapproved operational recovery
   plan. An old binary requires its matching old identities/data, never new
   bytes. Old data may now be stale: re-scrape/flag/refuse per existing policy.
   CHE-0022:84-86 is roll-forward/re-scrape, not automatic cross-major rollback.
   Verify repository, org and team routing as a unit before any rollback deploy.

## OCC response asymmetry: settled policy, separate proof obligation

Original ghr-5e5b7b20 described team warning/next-tick versus collection failure.
Current source and ratified text resolve the **response vocabulary** without
changing policy: fatal is scoped to the current run/tick, not daemon lifetime.
PGN-0016:35-39 permits cross-run recovery after abort/re-establish ownership/replay;
67-71 forbids tip-resync-and-retry **inside append**. CHE-0088:28,40-42 explicitly
requires both loops to use the consumer-owned convergence sink. A new attempt
need not wait for the next timer; the current implementation fast-rearms first.

Exact current callsites (relative to `crates/gh-report/src/`):

- `app/write_policy.rs:83-103,117-123`: FencedConflict -> Conflict -> Fatal;
  WARN is severity only, not permission to swallow or retry append.
- `app/team_refresh.rs:90-120,130-161`: write failure returns via `?`, stopping
  remaining tick work. `:169-180` is terminal logging, not the recovery policy.
- `app/collect.rs:1273-1278`: org record propagates typed failure before final
  rendering; `:1209-1213` propagates batch fence; `:418-447` returns distinct
  FencedConflict rather than Completed. `:405-407` releases run lock.
- `app/daemon.rs:727-745,776-799`: initial/scheduled collection calls
  `rearm_fenced_run`; `:571-575` resyncs then invokes a fresh run_with_outcome.
- `app/daemon.rs:925-949`: run_one_team_refresh_tick routes typed conflict to
  rearm_fenced_team_refresh_tick; `:687-692` resyncs then invokes a fresh tick
  function (reuses fetched_at; not evidence of a fresh timestamp).
- `app/daemon.rs:537-550,634-657`: both wrappers call converge_on_fence;
  `:487-519` requires successful resync before each run, bounded by existing
  policy `:429-433` (three attempts, two-second base), then returns give-up.
- `app/state/mod.rs:1591-1633`: reopens repository, org and team stores,
  sequentially, stopping on error; not an atomic three-store transaction.

Conclusion: no new severity/response decision is needed to preserve abort-current
plus consumer-level rearm. The stale claim that only team refresh re-arms is
false at these callsites. This is **source/document analysis**, not proof that
the resync implementation really re-establishes ownership under all interleavings.
Do not elevate comments asserting that property into executed evidence.

PGN-0021:27-41 still requires exhaustive safety/liveness modelling: losing append
adds no authoritative event, aborted writer cannot continue useful stale work,
fresh ownership/replay precedes next command, idempotency survives crash/ack-loss
and dedup expiry, serialization covers seq updates, and abort/replay eventually
progresses under explicitly stated fairness. Include repo/org/team concurrent
loops, partial resync failure and shutdown. Bounded sampling or this reference
table does not discharge it. No OCC model compiled or executed here.

## Executable evidence

From repo root: `python3.12 -S -B tools/test_commit_attribution_reference.py`.
Eight unit tests include the 49-pair system/Unknown no-attribution matrix. The
reference is intentionally isolated from production/native code; no integration
or E2E result follows from these tests. No E2E verify specified; unit verifies only.
