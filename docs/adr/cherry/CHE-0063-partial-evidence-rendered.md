# CHE-0063. Mid-Sweep Partial Evidence Rendered as Non-Terminal Event

Date: 2026-05-17
Last-reviewed: 2026-05-17

Tier: B
Status: Accepted

## Related

References: CHE-0054, CHE-0022, CHE-0024

## Context

CHE-0054:R1 collapsed `gh-report`'s sweep lifecycle into the `Run` aggregate with five run-scoped events. Invariant R1.c constrained `EvidencePublished` to follow `SweepCompleted`. In practice the application layer published *two* semantically distinct artefacts through that single command: the terminal end-of-sweep dashboard render, and a mid-sweep partial render driven by a debounce on the partial publisher. The mid-sweep call landed against `phase=Started` and the aggregate correctly rejected it (`RunError::NotCompleted(Started)`); the application layer logged a non-fatal warning on every real run. Two failure shapes hide in that warning: app layer violating its own aggregate, and one command carrying two intents. Adding a non-terminal variant for the second intent restores both the invariant and the type distinction.

## Decision

Introduce `DomainEvent::PartialEvidenceRendered` as a non-terminal `Run`-aggregate event admissible only while `phase == Started`, surfaced via a `RenderPartial` command. The application layer reorders so terminal `EvidencePublished` is emitted after `SweepCompleted`, and routes mid-sweep partial renders through `RenderPartial`.

R1 [5]: The `Run` aggregate emits `PartialEvidenceRendered` only from `phase == Started`; a `RenderPartial` command arriving from any other phase is rejected with `RunError::NotStarted`, mirroring the rejection shape of `RecordProgress` per CHE-0054:R1.d.

R2 [5]: `PartialEvidenceRendered::apply` is a no-op on `Run` phase; it records observability data only and leaves `phase`, `batch_id`, `repo_count`, and `completed` unchanged so the existing terminal-event ordering (CHE-0054:R1.b, R1.c) remains the sole driver of phase transitions.

R3 [5]: The application-layer finalize sequence emits `SweepCompleted` strictly before `EvidencePublished`; the helper that renders HTML and broadcasts the cache update is split from the helper that issues the terminal `PublishEvidence` command so the ordering can be enforced at the call-graph level.

R4 [5]: The mid-sweep partial publisher uses `RunService::render_partial`, never `publish_evidence`; the terminal command surface is reserved for the post-`SweepCompleted` call site.

R5 [5]: The warm-start cold-boot path synthesises a complete lifecycle triple `SweepStarted` → `SweepCompleted` → `EvidencePublished` under a `batch_id` prefixed `warm-start-`, each command fire-and-forget on `Err` per the existing non-fatal idiom; downstream consumers distinguish synthetic from real sweeps by the prefix.

R6 [5]: `PartialEvidenceRendered` is added as an additive `DomainEvent` variant per CHE-0022:R5 — no `EVIDENCE_SCHEMA_VERSION` bump, no msgpack reader change; the exhaustive `event_type()` match plus JSON+msgpack round-trip tests and the `event_type_matches_serde_tag` test compile-enforce the discriminator stability.

## Consequences

+ becomes easier: distinguishing terminal vs non-terminal evidence rendering at the type level; preserving CHE-0054:R1.c on every real run; reading the sweep lifecycle linearly in `step_finalize`.

− becomes harder: a downgraded binary reading an event log that contains `PartialEvidenceRendered` fails to deserialise the unknown variant — the standing additive-evolution constraint of CHE-0022:R5. The `warm-start-<ISO8601>` `batch_id` prefix is now a load-bearing convention; renaming it is a wire break.

risks/migration: existing logs are forward-compatible (no partial envelopes to break on); new logs are not readable by pre-ADR binaries — rollback past this ADR requires baseline + checkpoint discard. The defence-in-depth `warn!` text changed from "EvidencePublished publish failed, non-fatal" to "post-complete publish unexpectedly rejected"; log-message only, surfaces on bug not happy path.
