# PGN-0019. JetStream Stream-Level Schema Marker and Open-Gate

Date: 2026-06-25
Last-reviewed: 2026-06-25
Tier: B
Status: Accepted
Crates: pardosa, pardosa-nats

## Related

References: PGN-0004, PGN-0009, PGN-0010, PGN-0016, CHE-0022

## Context

JetStream stores need the same fail-closed schema gate that `.pgno` containers get from PGN-0004 R1, but JetStream has no container header. The stream-level home must be unconditional across the pinned async-nats feature set: server metadata remains unavailable because `server_2_10` is off, while stream `description` is always available. The gate must also preserve PGN-0010's split between substrate mechanics and adapter policy.

## Decision

JetStream stream descriptions carry the opaque schema marker written by `pardosa-nats`, while `pardosa` reads and compares it once while opening the typed store, before decoding the first frame.

R1 [5]: The JetStream schema marker lives in the stream `description`; `pardosa-nats` writes the configured opaque marker during stream provisioning, and no async-nats `server_2_10` metadata feature is required.
R2 [5]: `pardosa` reads and compares the marker once at `EventStore::<T>::create_with_backend` / `open_with_backend` time, before the first JetStream frame is decoded or folded into the frontier.
R3 [5]: A present matching marker opens; a present differing marker refuses with `SchemaHashMismatch`; an absent marker on a populated stream refuses with `SchemaMarkerAbsent`; an absent marker on an empty stream opens so the marker can be seeded by the typed store.
R4 [5]: Marker writes are substrate provisioning mechanics in `pardosa-nats`; marker comparison and fail-closed policy live in the `pardosa` adapter ring, preserving the PGN-0010 / PGN-0016 split.
R5 [5]: The marker is gate metadata only; it never contributes to the canonical bytes, envelope hash, frontier CRH, or event payload encoding.
R6 [5]: Reading the marker stays behind the synchronous `EventStore` facade; no public async store API is introduced.

## Consequences

+ becomes easier: JetStream opens now enforce the same single-schema-per-stream refusal posture as `.pgno` container opens.
− becomes harder: pre-existing populated streams without descriptions must be refused and re-scraped instead of silently opened.
risks/migration: the per-frame replay tag remains as defence-in-depth; live tests cover both differing-marker and absent-populated refusal paths; gh-report operators re-scrape refused streams under PGN-0009 / CHE-0022 policy.
