# PGN-0017. File-Mode Incremental Append and Torn-Tail Recovery

Date: 2026-06-16
Last-reviewed: 2026-06-16
Tier: B
Status: Accepted
Crates: pardosa-file, pardosa

## Related

References: PGN-0004, PGN-0006, PGN-0007, PGN-0005, CHE-0053, CHE-0072

## Context

PGN-0004 makes a finished `.pgno` a whole-file-validated container with raw index and footer. The file-mode write path needs O(delta) appends without replacing that finished shape. `AppendWriter` already streams bodies, fences footerless prefixes, and finishes byte-identical to `Writer::finish`; its manifest recovery API salvages the last manifest-covered prefix. The missing governance is the process-crash contract around that footerless window, the external sidecar lifecycle, and the typed recovery surface. Power-loss hardening, atomic rename, parent-directory fsync, and NATS rewrites are out of scope.

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
R6 [5]: After process crash, SIGTERM, or SIGKILL before finish, recovery reads
  the sidecar, validates header binding and body checksums, truncates bytes past
  manifest `data_end`, and writes the standard `.pgno` index plus footer.
R7 [5]: The durability scope is fdatasync/process-crash safety only: no atomic
  rename, no parent-directory fsync, and no power-loss claim are ratified here.
  CHE-0072:R6 remains unchanged because JetStream already uses per-event frames.
R8 [5]: The canonical generic file/runtime boundary surface for torn-tail
  recovery is `FileError::TornWriteRecovery { source: Box<RecoveryError> }`.
  `RecoveryError` remains the manifest API's detailed typed source; neither path
  may flatten recovery failures to generic `Io` or `String`.

## Consequences

+ becomes easier: file-mode writes can persist only the new event suffix while
  preserving the PGN-0004 finished container and reader contract.
− becomes harder: the runtime must create, sync, and cleanly ignore or remove a
  same-directory `.pgix` sidecar; direct `Reader::open` of a footerless prefix
  stays invalid by design.
risks/migration: sub-mission 02 must honour the typed surface in R8 and the
  sidecar path in R4. Observation is by `append_*recovery*` byte-identity and
  torn-tail tests, manifest layout tests, future runtime crash-recovery tests,
  and `adr-fmt --context pardosa` surfacing R1–R8.
