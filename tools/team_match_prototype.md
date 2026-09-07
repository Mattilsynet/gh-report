# Canonical team matching preparation — not deployed

`team_match_prototype.py` is an isolated, stdlib-only executable specification.
No production module imports it; it performs no IO, API fetch, persistence,
HTML rendering, lifecycle command, or clock read. No dependency/schema change.
Run: `python3.12 -S -B tools/test_team_match_prototype.py`.

## Input and result contract

- Original CODEOWNERS token, tuple of independent `TeamDto(slug, display_name)`,
  separate `Provenance(organization, observation, coverage)`, explicit `Limits`.
- The caller supplies provenance; this prototype does not authenticate a DTO
  or decide freshness. Positives mean observed in that snapshot, not live now.
- Exact ASCII slug: `Exact`. ASCII case-only spelling difference against a
  unique observed slug: `CaseDiscrepancy`. This is a spelling observation, not
  a claim GitHub rejects that spelling. Organization comparison is ASCII-folded.
- Display names never participate in matching or slug generation. Non-ASCII
  identifiers remain Unknown; no transliteration or Unicode equivalence guessed.
- Absence always yields `Unknown`, even if a caller labels coverage Complete.
  No Missing/Deleted verdict exists. Partial visibility, incomplete pagination,
   unauthorized access, malformed inputs, conflicting duplicate candidates, and exhausted
  bounds cannot become accusations. Unauthorized catalogs yield no positives.
- `ghr-bbobn` observed 37 visible teams under one credential, not all org teams.
  Tests use synthetic visible teams and an omitted secret team, not a copied
   authoritative organization catalog.

`Unknown.reason` must be an `UnknownReason` instance; construction and dataclass
replacement reject strings or other enums with `TypeError`. Frozen fields prevent
ordinary assignment, not deliberate Python reflection bypasses.
Identical full DTO duplicates deduplicate; different display names or canonical
casing for the same folded identity yield `AMBIGUOUS_IDENTITY`.

Validation intentionally returns the first failure: limits, provenance,
authorization, catalog shape/bound, owner syntax, organization, then entries
in supplied tuple order (entry validity before duplicate comparison), finally
not-observed. It is deterministic for a given ordered input, not permutation
invariant: a conflicting pair before a malformed entry yields ambiguous identity;
a malformed entry before the conflict yields invalid catalog entry. Tests pin both.

Verification: `python3.12 -S -B -m unittest discover -s tools -p 'test_*prototype.py'`
runs 26 methods: 17 in the team test module (including two cross-prototype
clock/constructor regressions) and 9 in the observation test module. The exact
reason table covers all nine enum members, including invalid catalog entries.

## Resource contract (prototype only)

One call, one raw token, already-allocated tuple: at most `max_teams` records
and `max_field_chars` characters per token, slug, display name, and provenance
field. Bounds are caller-selected test inputs, not production capacity policy.
Over-limit/unsupported iterable inputs yield Unknown without truncating or
consuming arbitrary generators. Work O(teams × field characters); temporary
strings O(field characters), one retained candidate; result references original
DTO/provenance. Input allocations, Python runtime/allocator and concurrent
callers are excluded; this is not a process-wide memory/byte bound. Caller
must bound acquisition bytes/items before allocating production catalogs.

## Future integration sites (unchanged)

- `src` paths below are relative to `crates/gh-report/`.
- `src/domain/codeowners.rs:36-57`: reuse `ParsedCodeowners.entries[].owners`
  raw spelling; missing/skipped/truncated input cannot prove all owners valid.
- `src/event/convert.rs:390-419`: raw owners already survive conversion both
  ways. No new raw-casing field is required.
- `src/domain/metrics.rs:585-610`: map keys lowercased; do not use
  `team_owner_slugs` normalized keys as original spelling for comparison.
- `src/collector/team_membership.rs:129-139`: current acquisition fetches
  members for requested slug, not canonical team identities. Any future
  bounded, budget-gated paginated catalog fetch belongs in collection/refresh,
  never rendering; no new fetch is implemented here.
- `src/report/html.rs:1390-1495`: owner view-model construction is a future
  join/presentation seam. Preserve observation age/coverage beside findings.
- `src/event/mod.rs:365-369,476-483`: durable org/team event shapes are a
  separate schema decision, not altered here.
- `src/collector/team_membership.rs:59-63,155-168`: cached 404 Deleted and
  downstream detach behavior remain separate unresolved lifecycle work.

## UI contract, not UI verification

All raw owner/display/canonical strings must remain untrusted text and pass
through contextual template escaping at the eventual UI boundary; never mark
them safe HTML or interpolate them into scripts. URL destinations need separate
validation/encoding. The malicious display-name test demonstrates stdlib HTML
text/quoted-attribute escaping only; it does not exercise Askama, a browser,
tooltips, or deployed UI. No XSS-safe production UI claim is made.

## Exact remaining decision

Select transient, non-durable catalog snapshots with explicit age, coverage,
credential-scope and refresh/restart semantics, OR durable canonical evidence
at the next scheduled schema-major cut with replay/cutover policy. No schema
cut solely for this prototype. Then authorize bounded acquisition, provenance
validation, Rust adaptation, view-model/template wiring and integration tests.
Neither choice permits absence from partial visibility to drive detach.
`ghr-mvtsb` remains open: the actual warning feature is not wired or shipped.
