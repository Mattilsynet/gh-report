# PAR-0021. Frontier Hash and Per-Fiber Hash Chain

Date: 2026-04-29
Last-reviewed: 2026-04-29
Tier: B
Status: Accepted

## Related

References: PAR-0003, PAR-0007, PAR-0012, GND-0005

## Context

PAR-0012 verifies precursor chains structurally — each event references
its predecessor's index. Structural linkage detects accidental gaps but
provides no tamper evidence: a malicious actor with write access to the
stream could rewrite history while preserving index continuity. SEC and
audit consumers need cryptographic non-repudiation.

BLAKE3 (256-bit, ~6× faster than SHA-256 on AVX2) gives per-event
hashing at ~50 ns on modern hardware — negligible against the 1M-event
verification cost. A per-fiber hash chain plus a stream-global
"frontier" hash (rolled forward across all events in append order)
gives both fiber-level and stream-level tamper evidence. Anchoring the
frontier to an external transparency log is then a small additional
publish per anchor interval.

## Decision

Pardosa adds a 32-byte BLAKE3 hash of the predecessor's canonical genome
bytes to every Event. Dragline maintains a 32-byte frontier hash rolled
forward in append order across all events in the stream. The frontier
is published periodically to a dedicated NATS subject for external
anchoring.

R1 [6]: Persist a precursor_hash field of length thirty-two bytes
  computed by BLAKE3 over the canonical genome bytes of the predecessor
  event in every Event<T>
R2 [5]: Set precursor_hash to all-zero bytes for the first event in a
  fiber so verification distinguishes fiber roots from chain breaks
R3 [6]: Maintain a Dragline::frontier hash of length thirty-two bytes
  updated by BLAKE3 chaining over each newly committed event in append
  order
R4 [6]: Publish the current frontier value to the NATS subject
  pardosa.{stream}.frontier on every anchor_interval tick for external
  transparency-log anchoring
R5 [5]: Reject any event during verify_precursor_chains whose
  precursor_hash does not match the BLAKE3 hash of the referenced
  predecessor's canonical bytes

## Consequences

Audit becomes cryptographically non-repudiable — any rewrite of stream
history is detectable at the next verification pass. External anchoring
extends the trust boundary beyond the JetStream cluster. Trade-offs: a
32-byte field on every event grows wire size; for typical 200-byte
envelopes this is a 16% overhead. Verification cost remains O(n) but
adds a hash compare per event. The frontier subject is an additional
publish path that consumers can subscribe to but is not itself
authoritative.
