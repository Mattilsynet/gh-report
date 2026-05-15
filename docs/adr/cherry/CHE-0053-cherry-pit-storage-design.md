# CHE-0053. Cherry Pit Storage Design

Date: 2026-05-09
Last-reviewed: 2026-05-09

Tier: B
Status: Accepted

## Related

References: CHE-0038, CHE-0007:R1, CHE-0001, CHE-0007:R2, CHE-0007:R3, CHE-0018:R1, CHE-0018:R3, CHE-0022:R1, CHE-0029:R1, CHE-0029:R5, CHE-0029:R6, CHE-0030:R1, CHE-0030:R2, CHE-0032, CHE-0036, CHE-0043, CHE-0051:R1

## Context

cherry-pit-storage is a new leaf-utility crate that absorbs the domain-agnostic, synchronous filesystem primitives from the legacy `quics-memoization` donor: `atomic_write_bytes` / `atomic_write_text` (temp + fsync + rename), `RunLock` with TTL-based stale detection, and `build_snapshot_signature` (canonical-JSON SHA-256). gh-report is the v0.1 consumer.

The crate sits on the synchronous side of the CHE-0018 boundary (CHE-0018:R1 sync, CHE-0018:R3 core async-free), opposite cherry-pit-runtime (CHE-0052). It carries no cherry-pit-* deps and per CHE-0029:R1 / CHE-0029:R4 stays a leaf peer of cherry-pit-core.

Invariant ownership stays with CHE-0032 (atomic writes), CHE-0036 (file-per-stream layout, gateway-owned), and CHE-0043 (process fencing); this crate ships *mechanisms* satisfying CHE-0032 and CHE-0043 only. Object-store backends (CHE-0044) and async wrappers are out of scope.

## Decision

cherry-pit-storage ships `PersistenceError`, `atomic_write_bytes`, `atomic_write_text`, `RunLock`, `LockMetadata`, `acquire`, `lock_path`, `DEFAULT_LOCK_FILENAME`, `DEFAULT_LOCK_TTL`, and `build_snapshot_signature` as a `pub use`-flat surface (CHE-0030:R1) over private `error`, `fs`, `lock`, and `signature` modules. The crate has zero cherry-pit-* dependencies and depends only on `thiserror`, `tempfile`, `tracing`, `serde`, `serde_json`, and `sha2` (plus whatever crates `lock.rs` already pulls in for filetime / process-id reading). The crate is synchronous — no tokio, no futures-util, no async fn anywhere on the public surface — placing it on the CHE-0018:R1 side of the sync/async line, opposite cherry-pit-runtime (CHE-0052). CHE-0032 / CHE-0036 / CHE-0043 are referenced (the mechanisms shipped here satisfy CHE-0032 and CHE-0043 invariants) but their text is not amended; ownership of the invariants stays with those ADRs. The DAG posture is no inbound cherry-pit-* edges in v0.1; `cherry-pit-gateway` retains its private atomic-write helper rather than depending on cherry-pit-storage, preserving the crate's leaf status (R12).

R1 [5]: cherry-pit-storage has zero cherry-pit-* dependencies — `[dependencies]` lists only `thiserror`, `tempfile`, `tracing`, `serde`, `serde_json`, `sha2`, and the donor's existing low-level deps (`filetime` for stale-lock detection, `gethostname` and `std::process` for `LockMetadata`); the crate MUST NOT add `cherry-pit-core` or any other `cherry-pit-*` crate to its `[dependencies]` table in v0.1

R2 [5]: cherry-pit-storage carries `#![forbid(unsafe_code)]` at the crate root per CHE-0007:R1 and CHE-0007:R3, contains no `unsafe` blocks, `unsafe impl`, or `unsafe fn` bodies per CHE-0007:R2 — the donor `quics-memoization` already conforms (it sets `#![forbid(unsafe_code)]` in `lib.rs:13`) and the surgical extract preserves the property; BC-14 is satisfied by construction

R3 [5]: cherry-pit-storage exposes its public API via private modules with selective `pub use` re-exports per CHE-0030:R1 — the flat surface is `PersistenceError`, `atomic_write_bytes`, `atomic_write_text`, `RunLock`, `LockMetadata`, `acquire`, `lock_path`, `DEFAULT_LOCK_FILENAME`, `DEFAULT_LOCK_TTL`, and `build_snapshot_signature`, and internal module structure (`error`, `fs`, `lock`, `signature`) is implementation detail per CHE-0030:R2 and may be reorganised without a SemVer-major bump as long as the re-export set is preserved

R4 [5]: cherry-pit-storage is synchronous — no `async fn`, no future-returning method, no tokio dependency, no futures-util dependency — placing the crate on the CHE-0018:R1 sync side of the boundary; consumers needing async I/O wrap calls in `tokio::task::spawn_blocking` themselves (per R7)

R5 [5]: cherry-pit-storage ships *mechanisms* satisfying CHE-0032 (atomic writes via temp + fsync + rename) and CHE-0043 (process-level fencing via TTL file locks), but does NOT own those invariants — CHE-0032 and CHE-0043 retain authoritative ownership and consumers cite them directly while reaching for `atomic_write_bytes` / `RunLock`; CHE-0036 layout stays gateway-owned

R6 [4]: cherry-pit-storage preserves the donor's documented crash-safety contract verbatim — `atomic_write_bytes` fsyncs file contents before rename but does NOT fsync the parent directory after rename, guaranteeing durability against process crashes but not against power-loss on all filesystems (per `quics-memoization::lib.rs` lines 8–11); changing this contract is a SemVer-major break and must come with its own ADR

R7 [4]: consumers requiring async I/O over storage primitives use `tokio::task::spawn_blocking(|| atomic_write_bytes(&path, &data))` or equivalent — cherry-pit-storage provides no async wrapper, no `AsyncStorage` trait, and no tokio integration in v0.1; a future v0.2 ADR may add a thin async-wrapping crate (e.g. `cherry-pit-storage-tokio`) above this one if the cost of `spawn_blocking` ceremony at consumer sites becomes load-bearing

R8 [4]: cherry-pit-storage MUST NOT be cited by `cherry-pit-core` (closure inflation via `tempfile` + `sha2` violates CHE-0029:R6); MAY be cited by `cherry-pit-gateway` subject to R12; `cherry-pit-projection`, `cherry-pit-web`, `cherry-pit-agent`, and `cherry-pit-runtime` MUST NOT cite this crate in v0.1 (their dep sets are fixed by CHE-0051:R1 and CHE-0052:R1)

R9 [4]: tests in cherry-pit-storage follow the CHE-0038 testing taxonomy — the donor's existing unit tests for atomic write semantics, lock acquisition / stale-detection / RAII release, and signature determinism are absorbed verbatim alongside the modules they exercise, and `tempfile::TempDir` is used as the standard test fixture for all filesystem interactions (no mocking of `std::fs`)

R10 [4]: the cherry-pit-storage public surface is additive-only across SemVer-minor bumps in the spirit of CHE-0022:R1 — adding new public types or re-exports is a SemVer-minor change, renaming or removing any item from the R3 enumeration is a SemVer-major break, and the `#[non_exhaustive]` marker on `PersistenceError` (preserved from the donor) keeps the enum surface extensible without breaking pattern-match exhaustiveness on consumers

R11 [4]: `build_snapshot_signature` is the only signature primitive shipped — it accepts `Option<&serde_json::Value>` and produces a 64-character lowercase hex SHA-256 over canonical JSON; alternative hashers (BLAKE3, SHA-512), Merkle structures, content-addressable storage backends, and any chunked / streaming signature variants are explicitly out of scope and deferred to a future ADR if any consumer surfaces a real need

R12 [5]: `cherry-pit-gateway` MUST NOT add `cherry-pit-storage` to its `[dependencies]` in v0.1; gateway retains its ~30 LOC private atomic-write helper as bounded intentional duplication. Rationale: (a) preserves leaf status (no inbound cherry-pit-* edges); (b) consolidation naturally reopens when R6's fsync-parent-dir contract changes; (c) adding the edge later is a trivial one-line reversible change

## Consequences

**Positive.** gh-report and future cherry-pit consumers gain a stable crate for crash-safe writes, run-locking, and signatures, severed from the dismantled `quics-memoization` lineage. Zero-cherry-pit-dep posture (R1) keeps the crate a leaf, independently publishable at v0.2. Synchronous stance (R4) leaves cherry-pit-core's transitive closure unchanged. CHE-0032 / CHE-0036 / CHE-0043 remain unamended (R5).

**Negative.** Fifth aspirational cherry-pit crate in `adr-fmt.toml`; adr-fmt warnings persist until A6 scaffolds it. R12's no-gateway-edge keeps two atomic-write implementations coexisting in v0.1 — intentional bounded duplication, both satisfying CHE-0032; consolidation reopens when R6's contract changes. Async-heavy consumers pay `spawn_blocking` ceremony per call site (R7).

**Open / deferred.** Object-store backends (CHE-0044), non-SHA-256 signatures (R11), async wrappers (R7), gateway consolidation (R12) all deferred.

## Rejected Alternatives

**Single combined `cherry-pit-infra` crate covering both runtime primitives and storage primitives.** Would have absorbed both `quics-aggregate` (the cherry-pit-runtime donor per CHE-0052) and `quics-memoization` (this crate's donor) into one workspace crate. Rejected because the two clusters have orthogonal dependency profiles: cherry-pit-runtime requires tokio and sits on the CHE-0018:R2 async-infrastructure side of the boundary; cherry-pit-storage is pure synchronous I/O on `std::fs` and sits on the CHE-0018:R1 sync side. Combining them would force every consumer of the storage primitives to take a tokio dependency for no benefit, inflating the BC-13 transitive closure of any storage-primitive consumer and violating the spirit of CHE-0029:R4's leaf-discipline reasoning. The mirror rejection appears in CHE-0052's Rejected Alternatives.

**Inline-absorb the storage primitives into gh-report.** Would have collapsed all four donor modules into `crates/gh-report/src/infra/` with no new cherry-pit crate. Rejected for the same reason cherry-pit-runtime's parallel inline-absorption was rejected (CHE-0052 first Rejected Alternative): the storage primitives are domain-agnostic by design (`atomic_write_bytes` and `RunLock` carry no notion of envelopes, aggregates, or events), and burying them inside an application crate forfeits any future cherry-pit consumer's ability to reuse them. cherry-pit-storage is the codified outcome.

**Take ownership of the CHE-0032 / CHE-0036 / CHE-0043 invariants and amend those ADRs to point at cherry-pit-storage as their authoritative home.** Would have re-tiered cherry-pit-storage upward and rewritten the three D-tier ADRs to delegate their guarantees to the new crate. Rejected because it conflates *invariant ownership* with *mechanism shipment*: CHE-0032 should remain the authoritative statement of "atomic file writes use temp + fsync + rename" regardless of whether cherry-pit-storage, cherry-pit-gateway's private helper, or a future object-store backend (CHE-0044) is the call-site mechanism. The split — invariants in their own ADRs, one mechanism in cherry-pit-storage — preserves substitutability (a CHE-0044 object-store backend can satisfy CHE-0032 differently without amending this ADR) and avoids the abort trigger named in the A3 mission contract (no existing CHE ADR is amended).

**Add cherry-pit-storage as an inbound dep on `cherry-pit-core` so core's `Aggregate` trait can call into atomic-write directly during apply.** Would have made cherry-pit-core a consumer of cherry-pit-storage. Rejected on two grounds: (i) it violates CHE-0018:R3 in spirit — core is supposed to be runtime-free and dep-minimal, and pulling `tempfile`, `sha2`, and `serde_json` into core's transitive closure for the benefit of a small mechanism subset is exactly the closure inflation CHE-0029:R6 guards against; (ii) it violates the CHE-0009 infallible-`apply` invariant — `Aggregate::apply` is sync and infallible, but `atomic_write_bytes` returns `Result<(), PersistenceError>` and is fallible by construction, so any temptation to call it from `apply` is already foreclosed by the type system. Storage-primitive consumers are infrastructure-tier (gateway, projection, agent, runtime, application-tier), never core.

**Ship advisory-lock (`flock`) instead of the donor's TTL-based file lock.** Would have replaced `RunLock` with a `std::os::unix::fs::FileExt`-based advisory lock acquisition. Rejected because the donor's TTL-based design is portable (works the same on macOS, Linux, and Windows without per-OS branches), survives the holding process being killed (the TTL allows recovery without operator intervention), and carries `LockMetadata` (host, PID, started-at) on disk so operators can inspect a stale lock. Advisory locks are released on process death (no recovery story for crashed-but-still-mapped state) and provide no on-disk introspection. The donor's choice is preserved.
