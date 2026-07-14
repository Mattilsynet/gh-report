//! [`MergerArm`] trait: the per-aggregate command-handling shape that
//! makes the merger aggregate-agnostic.
//!
//! Each consumer implements [`MergerArm`] for its aggregate's command
//! type, supplying [`MergerArm::persist_mode`] (the persist
//! strategy), [`MergerArm::handle`] (the pure load-then-handle step
//! against the merger's [`Aggregate::apply`]-replayed state), and
//! [`MergerArm::publish_label`] (the bus-failure log label).
//!
//! A trait, not an enum, because the merger cannot know any
//! consumer's command type at compile time and type-erasure would
//! defeat the single-aggregate compile-time guarantee; the consumer
//! writes the exhaustive matcher inside its own `handle` impl per
//! [CHE-0069:R2].
//!
//! [`MergerArm::Err`] must implement `From<StoreError>` so the merger
//! can lift persist-side failures (load, create, append, concurrency
//! conflict) into the arm's domain error shape uniformly; see
//! [CHE-0069:R5].
//!
//! [CHE-0069:R2]: https://github.com/acje/solon/blob/main/docs/adr/cherry/CHE-0069-cherry-pit-merger.md
//! [CHE-0069:R5]: https://github.com/acje/solon/blob/main/docs/adr/cherry/CHE-0069-cherry-pit-merger.md
//! [`StoreError`]: cherry_pit_core::StoreError
//! [`Aggregate::apply`]: cherry_pit_core::Aggregate::apply

use cherry_pit_core::{Aggregate, StoreError};

/// Per-command persist strategy returned by
/// [`MergerArm::persist_mode`].
///
/// The three variants correspond to the three persist shapes fixed
/// in [CHE-0069:R3]:
///
/// - [`PersistMode::Create`] — fresh aggregate per call, no
///   domain-key lookup or fold; the merger calls
///   [`EventStore::create`] directly against a default-constructed
///   state.
/// - [`PersistMode::CreateOrAppend`] — lazy create-or-append by
///   domain key: the merger looks the key up in the routing index,
///   folds prior envelopes if any, calls [`MergerArm::handle`], then
///   either [`EventStore::create`]s (first reference) or
///   [`EventStore::append`]s against the tracked `expected_sequence`
///   (subsequent references).
/// - [`PersistMode::AppendStrict`] — strict append requiring an
///   existing routing-index entry; a miss returns
///   [`MergerArm::missing_key_error`] without touching the store.
///
/// [CHE-0069:R3]: https://github.com/acje/solon/blob/main/docs/adr/cherry/CHE-0069-cherry-pit-merger.md
/// [`EventStore::create`]: cherry_pit_core::EventStore::create
/// [`EventStore::append`]: cherry_pit_core::EventStore::append
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PersistMode {
    /// Fresh aggregate per call. No domain-key lookup, no fold.
    Create,
    /// Lazy create-or-append by domain key. Missing key triggers the
    /// create-path; existing key triggers the append-path.
    CreateOrAppend(String),
    /// Strict append by domain key. Missing key returns
    /// [`MergerArm::missing_key_error`] without touching the store.
    AppendStrict(String),
}

/// Caller-supplied per-aggregate command handler.
///
/// One impl per (`Aggregate`, command-enum) pair. The trait's three
/// methods are pure functions — no I/O, no awaits — invoked by the
/// merger task between the load step and the persist step of every
/// command. The merger crate handles the load, the persist
/// (create-or-append per [`MergerArm::persist_mode`]), the
/// routing-index update, the per-aggregate sequence-tracker update,
/// and the publish-or-trace step.
///
/// `Send + Sync + 'static` is the merger's bound — the arm is held
/// for the lifetime of the merger task, which itself is
/// `'static` and may be polled across threads on a multi-threaded
/// runtime.
pub trait MergerArm<A: Aggregate>: Send + Sync + 'static {
    /// The caller's command type. Carried by [`MergerCommand`] across
    /// the [`mpsc`] boundary into the merger task.
    ///
    /// [`MergerCommand`]: crate::MergerCommand
    /// [`mpsc`]: https://docs.rs/tokio/latest/tokio/sync/mpsc/index.html
    type Cmd: Send + 'static;

    /// The caller's per-aggregate error type. Returned through the
    /// merger's `oneshot` reply channel verbatim. Must be
    /// constructible from [`StoreError`] so the merger can lift
    /// persist-side failures into the arm's error shape uniformly.
    type Err: From<StoreError> + Send + 'static;

    /// Choose the persist strategy for `cmd`. Pure; called before the
    /// load step.
    fn persist_mode(&self, cmd: &Self::Cmd) -> PersistMode;

    /// Apply `cmd` against the already-folded aggregate `state` and
    /// return the resulting events. Pure; called between the merger's
    /// load step and persist step. The merger has already constructed
    /// `state` from a default + replay of prior envelopes (empty for
    /// [`PersistMode::Create`] and miss-paths of
    /// [`PersistMode::CreateOrAppend`]).
    ///
    /// # Errors
    ///
    /// Returns [`MergerArm::Err`] when the aggregate rejects the
    /// command on an invariant violation. The merger surfaces this
    /// verbatim through the reply channel without consulting the
    /// store.
    fn handle(&self, state: &A, cmd: Self::Cmd) -> Result<Vec<A::Event>, Self::Err>;

    /// Static label for the bus-failure log record emitted from the
    /// publish-or-trace step. Per-command granularity matches the
    /// gh-report convention (e.g. `"SweepStarted"`,
    /// `"RepoEvaluated"`, `"WebhookReceived"`).
    fn publish_label(&self, cmd: &Self::Cmd) -> &'static str;

    /// Construct the error returned by the merger when a
    /// [`PersistMode::AppendStrict`] lookup misses the routing
    /// index. Default impl wraps a generic
    /// [`StoreError::CorruptData`] via the required
    /// `From<StoreError>` bound; override for a richer
    /// domain-specific shape (e.g. `RunError::RoutingMiss(key)` in
    /// gh-report).
    #[must_use]
    fn missing_key_error(&self, key: &str) -> Self::Err {
        Self::Err::from(StoreError::CorruptData(
            format!(
                "MergerArm::AppendStrict lookup miss on domain_key {key:?}; \
                 expected an existing routing-index entry"
            )
            .into(),
        ))
    }
}
