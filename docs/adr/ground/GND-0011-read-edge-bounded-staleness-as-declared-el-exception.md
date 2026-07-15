# GND-0011. Read-Edge Bounded Staleness as a Declared EL Exception to Write-Path PC/EC

Date: 2026-07-15
Last-reviewed: 2026-07-15
Tier: S
Status: Accepted

## Related

References: GND-0010, PGN-0016, PGN-0022, GND-0005, COM-0018

## Context

GND-0010 classifies pardosa and cherry-pit PC/EC and binds that
classification to every component, but R1 is scoped to the write path: the
single-writer-per-aggregate append boundary fenced by PGN-0016. It says
nothing about a cross-machine derived view — a remote memoization,
projection, or read model consumed away from the writer — which by
construction cannot observe the writer's latest committed state without its
own network round trip.

GND-0010's rejected Option 2 ("mixed per-operation tunability") ruled out
letting each call site pick an arbitrary, unaudited consistency point on the
write path. That does not extend to the read edge: a read model is already,
unavoidably, a fold over a log prefix rather than a live view of the writer.
Treating that as an undeclared defect (silently serving stale data as if
fresh) or forbidding it outright (every read synchronously consulting the
writer) are both wrong: the first hides staleness behind an implicit
freshness promise; the second turns every read into a write-path
linearization, defeating the point of a derived view.

A third option — declare, name, and enforce the staleness bound as an
explicit per-endpoint choice at the read edge only, leaving GND-0010's
write-path PC/EC untouched — is ratified here.

## Decision

A cross-machine derived view (remote memoization, projection, read model) is
an eventually-consistent, deterministic, read-only fold over a log prefix: a
consistent snapshot at all times, with a bounded and enforced recency bound —
never a claim to hold "the latest state at all times." This is a declared,
per-endpoint EL exception at the read edge; GND-0010's write-path PC/EC is
unchanged and remains binding on every append.

R1 [7]: Classify every cross-machine derived view's read contract as
  bounded-stale by default: reads may be served from a memoization lagging
  the authoritative writer, within an enforced, observed lag bound. A read
  from a projection beyond that bound is refused or flagged, never silently
  served as current.
R2 [7]: Require monotonic reads within a session: a client must never observe
  sequence N and later observe a sequence below N from the same read edge.
  The enforcement mechanism (client-carried high-water-mark token) is
  mechanism, not principle, and is delegated to the owning crate's ADR
  (PGN-0023).
R3 [7]: Treat strict, read-your-writes consistency as a per-request opt-in
  the caller must explicitly request, never a default or a global mode; a
  caller that does not opt in accepts bounded staleness.
R4 [3]: Forbid a derived view from ever authoring truth: it is a pure,
  read-only fold over the log. A memoization that writes back becomes a
  second writer against the aggregate and defeats the single-writer
  invariant (COM-0018) and the OCC fence (PGN-0016) GND-0010 R5 relies on.
R5 [5]: Require every implementation of this bounded-stale contract to
  satisfy GND-0005: the lag bound is not a design intent unless it is
  observed and reported, not merely assumed.
R6 [5]: Bind this principle on every read model in the pardosa and
  cherry-pit family; a crate-specific mechanism ADR (starting with
  PGN-0023 for pardosa/pardosa-nats) instantiates R1-R3 with concrete
  enforcement, never contradicts them.

## Consequences

- **GND-0010 is unchanged.** Write-path PC/EC still binds every append; this
  ADR adds a read-edge axiom, it does not relax or supersede GND-0010.
- **Read models gain a named contract.** Prior to this ADR, no ground-tier
  rule governed derived-view staleness; PGN-0022's applied-sequence
  high-water mark is retroactively understood as one instance of the R1
  enforcement signal this ADR requires generally.
- **Derived-views-never-author-truth is now a ground axiom** (R4), not an
  implicit assumption; COM-0018's single-writer invariant is the write-side
  half of the same guarantee this ADR completes on the read side.
- **Per-request RYW keeps the consistency surface auditable.** R3 avoids
  GND-0010's rejected Option 2 failure mode (unaudited per-call tunability)
  by scoping caller choice to an explicit, observable per-request flag
  rather than an ambient mode.
- **Retroactive cost.** A read model that silently claims freshness beyond
  its enforced lag bound, or that writes back into the log it folds over, is
  a defect under this ADR and must be reported per GND-0004.
</content>
