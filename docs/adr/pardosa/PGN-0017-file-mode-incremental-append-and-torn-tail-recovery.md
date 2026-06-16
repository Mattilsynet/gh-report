# PGN-0017. File-Mode Incremental Append and Torn-Tail Recovery

Date: 2026-06-16
Last-reviewed: 2026-06-16 — refined — admit torn-footer reader classes into auto-recovery with durable-region discriminator; ratify RecoveryOutcome surfacing and manifest-frontier placement (mission:pgno-crash-recovery-hardening)
Tier: B
Status: Accepted
Crates: pardosa-file, pardosa

## Related

References: PGN-0004, PGN-0006, PGN-0007, PGN-0005, CHE-0053, CHE-0072, AFM-0029

## Context

PGN-0004 makes a finished `.pgno` a whole-file-validated container. `AppendWriter` streams bodies, fences footerless prefixes, and finishes byte-identical to `Writer::finish`; manifest recovery salvages the last manifest-covered prefix. The missing governance is the crash contract for torn-footer reader errors, sidecar frontier state, and typed recovery observation.

The 2026-06-16 amendment follows AFM-0029's amend-in-place path; the rest of this ADR remains in force. R2's no-`FORMAT_VERSION` clause is `.pgno`-scoped; R10 introduces a separate `.pgix` manifest version and preserves `.pgno` footer, index, and fixture bytes.

## Decision

File-mode incremental append is an additive PGN-0004 protocol, not a new `.pgno` wire format. During an append session the writer may hold a footerless, `Reader::open`-incompatible prefix plus an external `PGIX` sidecar. Clean finish writes the existing index and footer; process-crash recovery validates the sidecar, truncates any unmanifested tail, and writes the same index and footer.

R1 [5]: Between sync and clean finish, an incremental writer appends only new
  stored message bodies; the footerless prefix is temporary writer state and is
  not a `Reader::open`-compatible `.pgno`.
R2 [5]: Clean finish writes the existing raw ascending index and footer
  byte-identical to `Writer::finish` for the same payload sequence. No
  `FORMAT_VERSION` bump, footer/index field change, or golden-fixture
  regeneration is required.
R3 [5]: Each durable append point syncs `.pgno` body bytes through
  `Syncable::sync_data` and then syncs the external manifest. A returned `Lsn`
  may cover only manifest-recorded body bytes; minting stays in the runtime sync
  path per PGN-0004:R4.
R4 [5]: The manifest sidecar is external, same-directory writer state at the
  `.pgno` path plus `.pgix` (for example `events.pgno.pgix`). It is created with
  the store, updated O(delta) per sync, and redundant after successful finish.
R5 [5]: The sidecar uses `PGIX` magic and `(offset, size, checksum)` records plus
  `data_end`. It is a recovery index only, not an authenticated anchor, and it
  must never be embedded inside `.pgno` bytes.
R6 [5]: After process crash, SIGTERM, or SIGKILL before finish, recovery
  auto-recovers only when a consistent `.pgix` proves every body in
  `[0, data_end]` intact and confines `InvalidMagic`, `InvalidIndex`,
  `InvalidChecksum`, or `InvalidReserved` to the post-`data_end` tail; it
  truncates that tail and writes the standard index and footer. In-region
  corruption is operator-gated, never auto-recovered.
R7 [5]: The durability scope is fdatasync/process-crash safety only: no atomic
  rename, no parent-directory fsync, and no power-loss claim are ratified here.
  CHE-0072:R6 remains unchanged because JetStream already uses per-event frames.
R8 [5]: The canonical generic file/runtime boundary surface for torn-tail
  recovery is `FileError::TornWriteRecovery { source: Box<RecoveryError> }`.
  `RecoveryError` remains the manifest API's detailed typed source; neither path
  may flatten recovery failures to generic `Io` or `String`.
R9 [5]: Recovery success returns its own `#[non_exhaustive]` `RecoveryOutcome`
  on the synchronous facade (PGN-0008:R1, PGN-0015:R6) and emits WARN telemetry.
  Recovery failure remains `FileError::TornWriteRecovery { source:
  Box<RecoveryError> }` per R8. Recovery is always surfaced as typed data, never
  silent.
R10 [5]: Persist the rolling unkeyed BLAKE3 frontier (`frontier_n =
  BLAKE3(frontier_{n-1} || body_n)`) in the `.pgix` manifest, never the `.pgno`
  footer, and assert it at recovery to bind exactly N ordered bodies to
  `data_end`; bump the `.pgix` manifest version and open frontier-absent
  manifests with the R6 checksum-only fallback, leaving `.pgno` bytes unchanged.

## Consequences

+ becomes easier: file-mode writes persist only the new event suffix while
  preserving the PGN-0004 container, reader contract, and `.pgno` wire bytes.
− becomes harder: the runtime maintains a versioned `.pgix` manifest with
  recovery frontier state; direct `Reader::open` of a footerless prefix stays
  invalid.
risks/migration: a footer exists only at true EOF after graceful close; no
  interior footer survives append or recovery. The frontier is tamper-evidence,
  not authentication, and never proves the last event committed; durability
  rides body+manifest fsync per PGN-0004:R4 plus out-of-band anchors. Power-loss
  hardening, atomic rename, parent-directory fsync, NATS rewrites, and `.pgno`
  fixture regeneration stay out of scope. Observation is by `adr-fmt --context`,
  manifest/frontier tests, recovery outcome checks, and WARN telemetry.
