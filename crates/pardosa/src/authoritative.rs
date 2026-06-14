//! Authoritative-storage backend handle surface (ADR-0022 §D1 /
//! §D11 / §D12).
//!
//! [`AuthoritativeBackend`] is the sealed marker identifying a
//! substrate eligible to back an [`crate::store::EventStore`] via
//! `EventStore::<T>::open_with_backend`. [`PgnoBackend`] wraps the
//! `.pgno`/`File` adapter (ADR-0006). Sealing is enforced via a
//! private `sealed::Sealed` supertrait; in-crate impls only.
//! `tests/ui/no_external_authoritative_backend_impl.rs` pins this.
//!
//! # Position vs. construction
//!
//! `open_with_backend` is the canonical adopter constructor when a
//! backend handle is in hand. Path constructors
//! (`EventStore::create`, `EventStore::open_validated`, …) remain
//! convenience wrappers. ADR-0022 §D12 admits only
//! `open_with_backend` to the audit allowlist.
use std::path::{Path, PathBuf};
/// Private sealed-trait root for [`AuthoritativeBackend`] (ADR-0022 §D1 / §D11).
///
/// Orthogonal to [`crate::backend::sealed::Sealed`]: admission
/// ([`AuthoritativeBackend`]) and behaviour
/// ([`crate::backend::BackendSink`]) are independent layers per
/// ADR-0022, so they seal under distinct private supertraits. A backend
/// can be admissible without being a sink implementor in principle.
mod sealed {
    pub trait Sealed {
        #[allow(
            private_interfaces,
            reason = "ADR-0022 §D1/§D11 sealed-trait pattern: the trait is private (in `sealed` mod) so exposing the pub(crate) BackendDispatch through __admit_into_dispatch never crosses the public adopter boundary"
        )]
        fn __admit_into_dispatch(self) -> super::BackendDispatch
        where
            Self: Sized;
    }
}
pub(crate) enum BackendDispatch {
    Pgno(PgnoBackend),
    JetStream(Box<jetstream::JetStreamBackendAdapter>),
    #[cfg(any(test, feature = "test-support"))]
    #[allow(
        dead_code,
        reason = "InMem admits the type tag but its payload is discarded by lifecycle.rs's _ pattern; the variant exists to prove admission, not to be rehydrated"
    )]
    InMem(fake::InMemoryBackend),
}
/// Crate-internal admission helper: extract the dispatch
/// discriminant from a sealed
/// [`AuthoritativeBackend`]-impl handle without crossing the
/// private `sealed` module boundary at call sites.
///
/// Uses fully-qualified syntax against the private
/// `sealed::Sealed::__admit_into_dispatch` supertrait method
/// (which all `AuthoritativeBackend` impls must provide), so
/// only in-crate code can drive admission while external
/// callers still see [`AuthoritativeBackend`] as a sealed
/// method-less marker (ADR-0022 §D1 / §D11; mission
/// `event-storage-dual-backend-20260606` sub-mission 05
/// admission seam).
pub(crate) fn admit_into_dispatch<B: AuthoritativeBackend>(backend: B) -> BackendDispatch {
    sealed::Sealed::__admit_into_dispatch(backend)
}
/// Sealed marker identifying a substrate eligible to back an
/// [`crate::store::EventStore`] via
/// `EventStore::<T>::open_with_backend` (ADR-0022 §D1, §D11).
///
/// Method-less: a type-system handshake that the substrate is
/// admitted. The behavioural contract (`append` / `sync`) lives on
/// [`crate::backend::BackendSink`] (ADR-0022 §D2); the two traits
/// compose at the adapter layer. Sealed via a private supertrait;
/// in-crate impls only.
pub trait AuthoritativeBackend: sealed::Sealed {}
/// `.pgno` path-backed [`AuthoritativeBackend`] handle (ADR-0022 §D11).
///
/// Opaque newtype around the journal path. Adopters obtain one via
/// [`PgnoBackend::open`] and feed it into
/// `EventStore::<T>::open_with_backend`. `open` does not touch the
/// filesystem; rehydration runs inside `open_with_backend`, reusing
/// the existing `.pgno` open path so framing, schema-hash, and
/// contiguity checks are preserved.
pub struct PgnoBackend {
    path: PathBuf,
}
impl PgnoBackend {
    /// Capture `path` as the substrate identifier for a future
    /// `EventStore::<T>::open_with_backend` call.
    ///
    /// Accepts anything `Into<PathBuf>` — both `&Path` and
    /// `PathBuf` work directly. No filesystem access here; errors
    /// (missing file, schema-hash mismatch, framing failure)
    /// surface at the `open_with_backend` site, not here.
    #[must_use]
    pub fn open(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}
impl sealed::Sealed for PgnoBackend {
    #[allow(
        private_interfaces,
        reason = "see sealed::Sealed declaration: trait is private, dispatch enum is pub(crate); admission stays in-crate"
    )]
    fn __admit_into_dispatch(self) -> BackendDispatch {
        BackendDispatch::Pgno(self)
    }
}
impl AuthoritativeBackend for PgnoBackend {}
/// Adopter-facing JetStream-backed [`AuthoritativeBackend`] handle
/// (ADR-0022 §D11; mirrors [`PgnoBackend`]'s opaque-wrapper shape).
///
/// Constructed via [`JetStreamBackend::open`] from a
/// [`pardosa_nats::JetStreamHandle`]. The wrapped adapter is
/// `pub(crate)` so adopters cannot reach the
/// [`crate::backend::BackendSink`] surface from outside — keeping
/// the substrate's write contract sealed and the "no adopter-facing
/// `JetStream` reader/cursor API" constraint pinned.
///
/// No I/O at construction time: the wrapped handle is lazy-connect.
pub struct JetStreamBackend {
    adapter: jetstream::JetStreamBackendAdapter,
}
impl JetStreamBackend {
    /// Wrap the supplied [`pardosa_nats::JetStreamHandle`]
    /// as the public sealed admission handle accepted by
    /// `crate::store::EventStore::<T>::open_with_backend`.
    ///
    /// Mirrors the `PgnoBackend::open` precedent: opaque
    /// newtype factory that captures the substrate handle
    /// without touching the underlying transport. The handle's
    /// own constructor already validated the offline
    /// configuration; this wrapper only stages it for the
    /// runtime's later `append` / `sync` / rehydrate dispatch.
    #[must_use]
    pub fn open(handle: pardosa_nats::JetStreamHandle) -> Self {
        Self {
            adapter: jetstream::JetStreamBackendAdapter::new(handle),
        }
    }
    /// Deprecated alias for [`JetStreamBackend::open`]; kept for
    /// `SemVer` compatibility with the pre-0.5 naming.
    #[must_use]
    #[deprecated(
        since = "0.5.0",
        note = "renamed to `open` for parity with PgnoBackend::open; use ::open instead"
    )]
    pub fn from_handle(handle: pardosa_nats::JetStreamHandle) -> Self {
        Self::open(handle)
    }
    pub(crate) fn into_adapter(self) -> jetstream::JetStreamBackendAdapter {
        self.adapter
    }
}
impl sealed::Sealed for JetStreamBackend {
    #[allow(
        private_interfaces,
        reason = "see sealed::Sealed declaration: trait is private, dispatch enum is pub(crate); admission stays in-crate"
    )]
    fn __admit_into_dispatch(self) -> BackendDispatch {
        BackendDispatch::JetStream(Box::new(self.into_adapter()))
    }
}
impl AuthoritativeBackend for JetStreamBackend {}
/// In-memory `AuthoritativeBackend` fake (cfg-gated; ADR-0022 §D11).
///
/// Exercises the sealed-trait surface from a non-`File` substrate
/// without widening either sealing supertrait. The matching
/// [`crate::backend::BackendSink`] impl lives in
/// [`crate::backend::fake`] so each fake submodule reaches its own
/// file-private `super::sealed::Sealed`.
///
/// Adopters reach the type as
/// `pardosa::store::test_support::InMemoryBackend`.
#[cfg(any(test, feature = "test-support"))]
pub(crate) mod fake {
    use super::AuthoritativeBackend;
    use super::sealed;
    /// In-memory authoritative-storage fake — a `Vec<u8>` substrate
    /// usable from in-tree tests (`cfg(test)`) and adopter tests
    /// (`feature = "test-support"`).
    ///
    /// Implements both [`AuthoritativeBackend`] (here) and
    /// [`crate::backend::BackendSink`] (in `crate::backend::fake`).
    /// The §D11 split-adapter pattern (sibling crate exports an
    /// opaque handle, `pardosa` owns the adapter wrapper) is
    /// reserved for the first real cross-crate backend; the in-tree
    /// fake composes both contracts on one type.
    pub struct InMemoryBackend {
        pub(crate) storage: Vec<u8>,
        pub(crate) synced_to: u64,
    }
    impl InMemoryBackend {
        /// Construct an empty in-memory backend with zero bytes
        /// staged and zero bytes acknowledged.
        #[must_use]
        pub const fn new() -> Self {
            Self {
                storage: Vec::new(),
                synced_to: 0,
            }
        }
        /// View of the bytes staged into the backend so far.
        #[must_use]
        pub fn bytes(&self) -> &[u8] {
            &self.storage
        }
    }
    impl Default for InMemoryBackend {
        fn default() -> Self {
            Self::new()
        }
    }
    impl sealed::Sealed for InMemoryBackend {
        #[allow(
            private_interfaces,
            reason = "see sealed::Sealed declaration: trait is private, dispatch enum is pub(crate); admission stays in-crate"
        )]
        fn __admit_into_dispatch(self) -> super::BackendDispatch {
            super::BackendDispatch::InMem(self)
        }
    }
    impl AuthoritativeBackend for InMemoryBackend {}
}
/// In-crate adapter shim wrapping the opaque
/// [`pardosa_nats::JetStreamHandle`] from the sibling substrate crate
/// (ADR-0022 §D10, §D11 "sealed trait + in-crate adapter shim").
///
/// `pardosa-nats` exports only the opaque handle and does **not**
/// impl [`AuthoritativeBackend`] or [`crate::backend::BackendSink`];
/// `pardosa` owns those impls here. No public symbol references the
/// `JetStream` concrete type; ADR-0022 §D12 audit allowlist stays
/// closed at `open_with_backend`.
///
/// The matching [`crate::backend::BackendSink`] impl lives in
/// [`crate::backend::jetstream`] so each adapter submodule reaches
/// its own file-private `super::sealed::Sealed`.
pub(crate) mod jetstream {
    use super::AuthoritativeBackend;
    use super::sealed;
    use pardosa_nats::JetStreamHandle;
    /// In-crate adapter wrapping a [`JetStreamHandle`] so the
    /// `JetStream` substrate participates in the sealed
    /// [`AuthoritativeBackend`] + [`crate::backend::BackendSink`]
    /// surface without the sibling substrate crate impl'ing
    /// either trait (ADR-0022 §D11).
    ///
    /// Constructed via [`Self::new`] from a handle returned by
    /// [`pardosa_nats::JetStreamBackend::open`]. The wrapped
    /// handle is exposed only via [`Self::handle`] for in-crate
    /// inspection; cross-crate code never names the wrapped type.
    pub(crate) struct JetStreamBackendAdapter {
        pub(crate) handle: JetStreamHandle,
        pub(crate) schema_tag: Option<String>,
    }
    impl JetStreamBackendAdapter {
        /// Wrap the supplied [`JetStreamHandle`] as the in-crate
        /// adapter the runtime drives the `JetStream` substrate
        /// through.
        ///
        /// Mirrors the in-tree
        /// [`super::fake::InMemoryBackend::new`] constructor
        /// shape: no I/O, no runtime activation. The handle's
        /// own constructor already validated the offline
        /// configuration; this wrapper only stages the handle
        /// for the runtime's later `append` / `sync` dispatch
        /// (sub-mission 02 wires the dispatch bodies; the
        /// detached-for-tests runtime handle traps any premature
        /// network call there).
        pub(crate) const fn new(handle: JetStreamHandle) -> Self {
            Self {
                handle,
                schema_tag: None,
            }
        }
        pub(crate) fn set_schema_tag(&mut self, schema_tag: String) {
            self.schema_tag = Some(schema_tag);
        }
        /// Borrow the wrapped [`JetStreamHandle`] for in-crate
        /// inspection (config probing in tests; the runtime's
        /// future `append` / `sync` dispatch in sub-mission 02).
        #[cfg(test)]
        pub(crate) const fn handle(&self) -> &JetStreamHandle {
            &self.handle
        }
    }
    impl sealed::Sealed for JetStreamBackendAdapter {
        #[allow(
            private_interfaces,
            reason = "see sealed::Sealed declaration: trait is private, dispatch enum is pub(crate); admission stays in-crate"
        )]
        fn __admit_into_dispatch(self) -> super::BackendDispatch {
            super::BackendDispatch::JetStream(Box::new(self))
        }
    }
    impl AuthoritativeBackend for JetStreamBackendAdapter {}
}
#[cfg(test)]
mod jetstream_adapter_shim_tests {
    use super::AuthoritativeBackend;
    use crate::authoritative::jetstream::JetStreamBackendAdapter;
    use crate::backend::BackendSink;
    use pardosa_nats::{JetStreamBackend, JetStreamConfig, RuntimeHandle};
    fn detached_config(tag: &str) -> JetStreamConfig {
        JetStreamConfig::builder()
            .stream_name(format!("shim-{tag}"))
            .subject(format!("shim.{tag}"))
            .durable_consumer(format!("shim-c-{tag}"))
            .runtime_handle(RuntimeHandle::detached_for_tests())
            .build()
            .expect("offline config is valid")
    }
    #[test]
    fn adapter_satisfies_authoritative_backend_marker() {
        fn requires_authoritative_backend<B: AuthoritativeBackend>(_: &B) {}
        let handle = JetStreamBackend::open(detached_config("marker-ab"));
        let adapter = JetStreamBackendAdapter::new(handle);
        requires_authoritative_backend(&adapter);
    }
    #[test]
    fn adapter_satisfies_backend_sink_trait() {
        fn requires_backend_sink<S: BackendSink>(_: &S) {}
        let handle = JetStreamBackend::open(detached_config("marker-bs"));
        let adapter = JetStreamBackendAdapter::new(handle);
        requires_backend_sink(&adapter);
    }
    #[test]
    fn adapter_constructor_consumes_handle_and_preserves_config_view() {
        let handle = JetStreamBackend::open(detached_config("config-view"));
        let adapter = JetStreamBackendAdapter::new(handle);
        let cfg = adapter.handle().config();
        assert_eq!(cfg.stream_name(), "shim-config-view");
        assert_eq!(cfg.subject(), "shim.config-view");
        assert_eq!(cfg.durable_consumer(), "shim-c-config-view");
        assert!(
            cfg.runtime_handle().is_detached_for_tests(),
            "detached test handle round-trips through the adapter"
        );
    }
}
