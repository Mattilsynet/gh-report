# CHE-0060. HashChainedEventStore Extension Trait

Date: 2026-05-16
Last-reviewed: 2026-05-18
Tier: B
Status: Accepted

## Related

References: CHE-0057, CHE-0016, PAR-0021, SEC-0011, CHE-0064

## Context

PAR-0021 specifies a per-fiber BLAKE3 hash chain (`precursor_hash` 32
bytes per event) and a stream-global `Dragline::frontier` for
cryptographic tamper evidence beyond CHE-0016's structural envelope.
PAR-0021 is Accepted at HEAD but unimplemented in pardosa source; the
current substrate exposes `Dragline::verify_precursor_chains()` as a
zero-argument whole-stream check; sub-stream verification is undefined
at the substrate layer and out of scope for this ADR. SEC-0011's
non-repudiation requirements are deferred to a later phase. The file-
backed adapter has no hash-chain capability. Forcing every EventStore
impl to carry hash-chain methods violates CHE-0057:R1 and would require
either unimplemented stubs (forbidden by CHE-0057:R3) or core-trait
bloat. An opt-in extension trait, with a documented always-failing
rollout stub permitted by CHE-0057:R3's PAR-0021 carve-out, matches the
substrate trajectory.

## Decision

R1 [5]: HashChainedEventStore extends EventStore as a supertrait bound
  per CHE-0057:R2 and lives in cherry-pit-core; standalone definition
  is forbidden.

R2 [5]: HashChainedEventStore exposes exactly two methods:
  frontier_hash returning a 32-byte BLAKE3 hash per PAR-0021:R3,
  and verify_chain validating the precursor_hash chain over the
  whole stream per PAR-0021:R5. Sub-stream verification is not
  exposed today; if a substrate later supports it, a superseding
  ADR introduces the shape (CHE-0057:R5 append-only).

R3 [5]: PardosaEventStore MAY implement HashChainedEventStore as an
  always-failing rollout stub returning StoreError::Infrastructure
  from both methods until PAR-0021 lands in pardosa source per the
  CHE-0057:R3 named carve-out; the stub MUST be documented in the
  impl block and MUST be removed when PAR-0021 lands.

R4 [5]: Substrates without a hash-chain capability MUST NOT implement
  HashChainedEventStore per CHE-0057:R3 outside the R3 rollout-stub
  carve-out; the file-backed cherry-pit-storage adapter is the
  primary non-implementer.

R5 [5]: Downstream code requiring hash-chain verification MUST bound on
  HashChainedEventStore per CHE-0057:R4; SEC-0011 consumers when
  un-deferred bind to this trait, not to EventStore directly.

## Consequences

Pardosa's PAR-0021 capability becomes surfaceable through the cherry-
pit port without committing every adapter to a non-existent surface.
The rollout-stub carve-out (R3) is time-bound by PAR-0021's eventual
implementation; the stub's removal triggers no ADR edit because the
signatures are append-only per CHE-0057:R5. SEC-0011 non-repudiation
remains formally deferred — this ADR provides the trait shape but no
claim of non-repudiation until pardosa implements PAR-0021 and a
follow-up SEC ADR ratifies the claim.

Encoding locality (scope: frontier-hash production at the
HashChainedEventStore trait-output boundary). R2's `[u8; 32]`
signature confines *frontier* hashing to the trait's output, not its
inputs; the adapter crate (e.g. cherry-pit-pardosa) chooses any
encoding strategy needed to produce a frontier hash. This paragraph
does NOT govern substrate-internal Encode bounds: PAR-0021:R5's
per-event `precursor_hash` is computed inside `pardosa::Dragline`
and requires `EventEnvelope<E>: Encode` / `AggregateId: Encode` at
the substrate layer — that locality is set by CHE-0064 (2026-05-18),
which amends CHE-0029:R4 to admit `pardosa-encoding` in
`cherry-pit-core` for exactly this purpose. The two surfaces are
distinct; this ADR's no-Encode-impl statement stands for frontier
hashing only.
