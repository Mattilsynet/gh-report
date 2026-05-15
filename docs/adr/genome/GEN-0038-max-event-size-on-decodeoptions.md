# GEN-0038. Max Event Size on DecodeOptions

Date: 2026-05-15
Last-reviewed: 2026-05-15
Tier: A
Status: Accepted

## Related

References: GEN-0013, GEN-0015, GEN-0035

## Context

GEN-0013's 256 MiB gross gate bounds whole files and whole compressed
messages. The v2 typing refresh introduces *single events* as a
first-class wire object via the bare-message header
(`crates/pardosa-genome/src/format.rs:65–78`):

```text
Uncompressed (23 bytes; v2):
  [format_version:u16][schema_hash:u128][algo:u8][msg_data_size:u32][data...]
Compressed (27 bytes; v2):
  [format_version:u16][schema_hash:u128][algo:u8][compressed_size:u32][msg_data_size:u32][data...]
```

`msg_data_size:u32` is the declared event-payload size — the value a
future bare-message decoder multiplies against an allocator. A crafted
4 GiB declared size inside a compressed stream that itself fits the
256 MiB gate still triggers a 4 GiB allocation absent a per-event cap.

Oracle K3 verdict (`adr-fmt-1s6c`) selected a runtime parameter over a
header field: the cap travels with decoder configuration, not the
bytes, leaving header layout frozen. Placement on the (existing)
`pardosa-genome::config::DecodeOptions` rather than a new struct in
`pardosa-encoding` resolved at execution time: genome's struct already
holds sibling ceilings (`max_message_size: 256 MiB`), and
`pardosa-encoding` is `#![no_std]` substrate with no options shape.

## Decision

Add a `max_event_size: u32` field to
`pardosa_genome::config::DecodeOptions`. Default **16 MiB**
(`16 * 1024 * 1024`). The field records a runtime cap, intended to
gate `msg_data_size <= options.max_event_size` (and equivalently
`compressed_size <= options.max_event_size`) before allocation in the
bare-message decode path.

Header layout in `crates/pardosa-genome/src/format.rs:11–35` is
unchanged — `max_event_size` is a runtime parameter, not a wire field.

R1 [4]: `max_event_size: u32` is a field on
  `pardosa_genome::config::DecodeOptions`, not a wire-format field
R2 [4]: The default value is 16 MiB, smaller than GEN-0013's 256 MiB
  gross gate, so the per-event cap is the tighter binding constraint
  in default configurations
R3 [4]: The on-disk header layout for bare messages and files is
  unchanged by this ADR (oracle K3 verdict `adr-fmt-1s6c`)
R4 [4]: Forward-compatibility (GEN-0015) is preserved because no wire
  bytes are added, removed, or reinterpreted; readers built before
  this ADR remain bit-compatible with readers built after

## Wire-in

`max_event_size` is **prospective** in this commit. The bare-message
decode path that would consume `msg_data_size` does not yet exist in
`pardosa-genome`; `msg_data_size` is documented in `format.rs:69–72`
as the wire field but no live code reads it. The runtime cap is
recorded in DecodeOptions for the future decode path to consult.
The validation call site (`msg_data_size <= options.max_event_size`)
lands when the bare-message decode capability is introduced, tracked
as a separate follow-up effort. The field's doc-comment carries the
same deferral note so a future reader does not assume the check is
already wired.

## Consequences

- **Positive:** Per-event allocation is bounded by a runtime policy
  finer than the gross 256 MiB ceiling, closing allocation-amplification
  vectors that the gross gate alone leaves open.
- **Positive:** No wire-format change; v2 readers and writers built
  before and after this ADR remain bit-compatible (GEN-0015).
- **Positive:** Co-located with sibling ceilings (`max_message_size`,
  `max_uncompressed_size`) on the same struct — one place to tune
  decoder DoS posture.
- **Negative:** Record-keeping only in this commit; the check is not
  enforced until the bare-message decode path lands. A decoder that
  does not consult `max_event_size` provides no protection. The
  doc-comment + §"Wire-in" reduce but do not eliminate the foot-gun.
- **Negative:** `u32` caps the per-event size at 4 GiB declared,
  matching `msg_data_size: u32` on the wire. Events above 4 GiB are
  out of scope for the v2 format.
