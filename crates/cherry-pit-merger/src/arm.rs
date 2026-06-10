//! [`MergerArm`] trait: the per-aggregate command-handling shape that
//! makes the merger aggregate-agnostic.
//!
//! Each consumer crate implements [`MergerArm`] for its aggregate's
//! command type. The trait carries three pieces:
//!
//! - [`MergerArm::persist_mode`] — for a given command, return the
//!   persist strategy ([`PersistMode::Create`],
//!   [`PersistMode::CreateOrAppend`], [`PersistMode::AppendStrict`]).
//! - [`MergerArm::handle`] — the pure load-then-handle step. The
//!   merger has already replayed the aggregate's prior envelopes
//!   through [`Aggregate::apply`] and now hands the folded state to
//!   the arm with the consumed command.
//! - [`MergerArm::publish_label`] — the static label included in
//!   bus-failure log records (per-command granularity matches the
//!   gh-report convention: `"SweepStarted"`, `"RepoEvaluated"`, …).
//!
//! ## Why a trait, not an enum
//!
//! The merger crate cannot know any consumer's command enum at compile
//! time, and a `Box<dyn Any>`-style erasure would defeat
//! [CHE-0005:R1]'s single-aggregate compile-time guarantee. A trait
//! parameterised on `A: Aggregate` keeps the dispatch monomorphic and
//! lets the consumer write the exhaustive matcher inside its own
//! [`handle`](MergerArm::handle) impl — exactly the
//! [CHE-0017:R2] caller-writes-the-matcher pattern that
//! [CHE-0051:R4]'s policy dispatch closure already follows.
//!
//! ## Error composition
//!
//! [`MergerArm::Err`] must be constructible from [`StoreError`] via a
//! `From` bound so the merger can lift persist-side failures (load,
//! create, append, concurrency conflict) into the arm's domain error
//! shape uniformly. The caller therefore implements one
//! `impl From<StoreError> for MyAggregateError` and the merger's
//! reply channel returns `Result<(), MyAggregateError>` end-to-end.
//!
//! [CHE-0005:R1]: https://github.com/acje/solon/blob/main/docs/adr/cherry/CHE-0005-single-aggregate-design.md
//! [CHE-0017:R2]: https://github.com/acje/solon/blob/main/docs/adr/cherry/CHE-0017-policy-output-static-type.md
//! [CHE-0051:R4]: https://github.com/acje/solon/blob/main/docs/adr/cherry/CHE-0051-cherry-pit-agent-design.md
//! [`StoreError`]: cherry_pit_core::StoreError
//! [`Aggregate::apply`]: cherry_pit_core::Aggregate::apply

use cherry_pit_core::{Aggregate, StoreError};

/// Per-command persist strategy returned by
/// [`MergerArm::persist_mode`].
///
/// The three variants correspond to the three triad shapes lifted
/// verbatim from the original gh-report Merger arms:
///
/// - [`PersistMode::Create`] — fresh aggregate per call (write-once
///   degenerate domains such as webhook ingest per [CHE-0054:R3]).
///   No domain-key lookup, no fold; the merger calls
///   [`EventStore::create`] directly with the events produced by
///   [`MergerArm::handle`] against a default-constructed aggregate
///   state.
/// - [`PersistMode::CreateOrAppend`] — lazy create-or-append by
///   domain key (repository evaluation per [CHE-0054:R2]). The
///   merger looks the key up in the routing index, folds prior
///   envelopes if any, calls [`MergerArm::handle`], then either
///   [`EventStore::create`]s + `or_insert`s the assigned id
///   (first-reference create-path) or [`EventStore::append`]s with
///   the caller-tracked `expected_sequence` (subsequent-reference
///   append-path).
/// - [`PersistMode::AppendStrict`] — strict append against an
///   existing aggregate (run-lifecycle commands after
///   `SweepStarted`, per [CHE-0054:R1]). The merger requires a hit
///   in the routing index; a miss returns
///   [`MergerArm::missing_key_error`] without touching the store.
///
/// [CHE-0054:R1]: https://github.com/acje/solon/blob/main/docs/adr/cherry/CHE-0054-gh-report-aggregate-decomposition.md
/// [CHE-0054:R2]: https://github.com/acje/solon/blob/main/docs/adr/cherry/CHE-0054-gh-report-aggregate-decomposition.md
/// [CHE-0054:R3]: https://github.com/acje/solon/blob/main/docs/adr/cherry/CHE-0054-gh-report-aggregate-decomposition.md
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
