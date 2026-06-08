use tokio::runtime::Handle;
/// Caller-supplied async runtime handle the `JetStream` client
/// will block on (ADR-0022 §D7 — required, no defaults; a
/// process-global runtime is explicitly rejected).
///
/// The wrapper exists so the public surface this crate exports is
/// the *role* (a handle the `JetStream` client drives on), not the
/// underlying concrete client-runtime crate. Today the inner
/// value is [`tokio::runtime::Handle`]; future revisions may
/// admit an `async-std` / `smol` shape behind the same wrapper
/// without a breaking change for adopters.
#[derive(Clone, Debug)]
pub struct RuntimeHandle {
    inner: Inner,
}
#[derive(Clone, Debug)]
enum Inner {
    Tokio(Handle),
    DetachedForTests,
}
impl RuntimeHandle {
    /// Wrap a caller-supplied [`tokio::runtime::Handle`].
    ///
    /// The caller owns the runtime's lifetime; this wrapper
    /// stores a clone and does not start, stop, or otherwise
    /// manage the runtime (ADR-0022 §D7).
    #[must_use]
    pub fn from_tokio(handle: Handle) -> Self {
        Self {
            inner: Inner::Tokio(handle),
        }
    }
    /// Detached placeholder usable from tests that exercise the
    /// substrate's offline shape only (config validation, handle
    /// construction).
    ///
    /// This variant carries no runtime; it must not be passed to
    /// any operation that would actually drive a `block_on`. The
    /// in-crate adapter shim (Phase 4 sub-mission 4.2) checks
    /// for this variant at the first network operation and
    /// surfaces a typed error rather than silently no-op'ing.
    #[must_use]
    pub fn detached_for_tests() -> Self {
        Self {
            inner: Inner::DetachedForTests,
        }
    }
    /// `true` if this handle is the test-only detached
    /// placeholder.
    #[must_use]
    pub fn is_detached_for_tests(&self) -> bool {
        matches!(self.inner, Inner::DetachedForTests)
    }
    /// Borrow the underlying [`tokio::runtime::Handle`] when
    /// this wrapper was constructed via [`Self::from_tokio`].
    /// Returns `None` for [`Self::detached_for_tests`].
    ///
    /// The in-crate adapter shim (Phase 4 sub-mission 4.2) calls
    /// this from the runtime crate to obtain the handle it
    /// `block_on`'s the `JetStream` client against.
    #[must_use]
    pub fn as_tokio(&self) -> Option<&Handle> {
        match &self.inner {
            Inner::Tokio(h) => Some(h),
            Inner::DetachedForTests => None,
        }
    }
}
