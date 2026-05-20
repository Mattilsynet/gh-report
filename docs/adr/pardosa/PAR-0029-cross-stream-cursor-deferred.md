# PAR-0029. Cross-Stream Cursor Deferred Beyond v0.1

Date: 2026-05-20
Last-reviewed: 2026-05-20
Tier: B
Status: Accepted

## Related

References: PAR-0016, PAR-0017, PAR-0021, CHE-0029, CHE-0065

## Context

The pardosa runtime exposes per-stream cursors (`Dragline<T>` per
stream, frontier hash per stream, per-fiber hash chain per
PAR-0021:R4). Multi-stream consumers — projections that subscribe to
events from more than one stream, replays that need a deterministic
join order across streams, audit tooling that walks the workspace's
durable history — need a cross-stream cursor: a value that uniquely
identifies a point in the partial order of all streams a consumer is
tracking, and against which the runtime can resume. PAR-0016
specified per-stream timestamps; PAR-0021 specified per-stream
frontier hashes; neither specifies a composite cursor. v0.1 needs an
explicit "this is out of scope" statement so consumers do not invent
ad-hoc encodings that conflict with the eventual blessed shape.

## Decision

R1 [5]: The v0.1 pardosa runtime does not expose a cross-stream
  cursor type or API. Consumers that today track multiple streams
  hold per-stream cursors explicitly and join them at the consumer
  layer; the runtime offers no convenience shape for this.

R2 [5]: Ad-hoc encodings of "all stream positions at time T" by
  external consumers are explicitly outside the v0.1 wire contract.
  When the cross-stream cursor lands, it will not be byte-compatible
  with any such encoding. External users who need cross-stream
  positions today are advised to keep their join encoding internal
  and replaceable.

R3 [5]: The cross-stream cursor design is deferred to a follow-up
  ADR. That ADR will address: composite identity (which streams are
  carried), monotonicity guarantees across streams (PAR-0016
  per-stream timestamps are not totally ordered), resumption
  semantics on broker reconnect, and interaction with the per-stream
  frontier publication (PAR-0021:R4).

R4 [6]: STORY and the pardosa crate README mark cross-stream cursors
  as deferred and cite this ADR; users opening the surface looking
  for the API find the deferral statement rather than absence.

## Consequences

+ becomes easier: v0.1 scope stays bounded; multi-stream consumer
  patterns surface their own join code rather than waiting for a
  runtime feature that is not coming.

− becomes harder: multi-stream replay and projection workloads carry
  per-stream cursor bookkeeping at the consumer layer until the
  follow-up ADR lands.

risks/migration: when the cross-stream cursor ADR lands, consumers
  using ad-hoc encodings (R2 warning notwithstanding) will need a
  one-time migration. The v0.1 deferral statement is the warning;
  the migration cost is owned by consumers who ignored it.
