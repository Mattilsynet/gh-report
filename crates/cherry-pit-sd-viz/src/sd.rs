//! Generic, host-testable systems-dynamics core (STELLA/iThink
//! vocabulary), pure Rust with zero `web-sys`/`wasm` leakage and zero
//! gh-report-specific types (adr-fmt-lmfyp, per the reference glossary
//! in adr-fmt-0pe95).
//!
//! Primitives, per adr-fmt-0pe95 sect 1:
//!
//! - [`Stock`] — the state variable; the integral of its net flows.
//!   `Stock(t+dt) = Stock(t) + dt * (inflow - outflow)`, integrated by
//!   Euler's method via [`Stock::step`].
//! - [`Flow`] — [`Uniflow`](Flow::Uniflow) clamps to non-negative;
//!   [`Biflow`](Flow::Biflow) may reverse (go negative). Direction
//!   ([`FlowDirection`]) is the caller's concern when composing net
//!   flow, not a property stored on the flow itself.
//! - [`Converter`] — auxiliary, no-state; wraps an algebraic function
//!   recomputed every call, never accumulating.
//! - [`Connector`] — an information-only value snapshot; holds a
//!   copied `f64`, never a handle back to the [`Stock`] it was read
//!   from, so it structurally cannot mutate material state.
//! - [`Terminal`] — model-boundary cloud terminals (Source/Sink); no
//!   state, excluded from conservation checks.
//! - [`LevelHistory`] — a bounded ring buffer recording recent samples
//!   of any one stock's level, oldest to newest, for "last N ticks"
//!   sparkline rendering. App-agnostic: it stores `f64` samples, not
//!   any particular stock's identity.
//!
//! Loop polarity (adr-fmt-0pe95 sect 2): [`loop_polarity`] classifies
//! a causal loop as reinforcing (R, even negative links) or balancing
//! (B, odd negative links).

/// The state variable of a system-dynamics model: the integral of its
/// net flows over time. See module docs for the Euler update equation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stock {
    level: f64,
}

impl Stock {
    #[must_use]
    pub fn new(initial: f64) -> Self {
        Self { level: initial }
    }

    #[must_use]
    pub fn level(&self) -> f64 {
        self.level
    }

    /// Advances this stock by one Euler integration step:
    /// `level += dt * net_flow`, where `net_flow` is the caller's
    /// precomputed `inflow - outflow` for this step.
    pub fn step(&mut self, dt: f64, net_flow: f64) {
        self.level += dt * net_flow;
    }
}

/// A bounded ring buffer of `f64` samples, oldest to newest, sized for
/// "last N ticks" sparkline rendering of a recorded level over time.
/// App-agnostic: it has no notion of which stock (or anything else)
/// a sample came from — callers own that mapping.
///
/// Capacity zero is not a meaningful window, so [`LevelHistory::new`]
/// clamps it to one: the most reversible choice, since a
/// single-capacity history still behaves correctly (always holds
/// exactly the latest sample) rather than panicking or silently
/// discarding every push.
#[derive(Debug, Clone)]
pub struct LevelHistory {
    samples: std::collections::VecDeque<f64>,
    capacity: usize,
}

impl LevelHistory {
    /// Creates an empty history with room for `capacity` samples.
    /// `capacity` of `0` is clamped to `1`.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            samples: std::collections::VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    /// Records `level` as the newest sample. When already at capacity,
    /// the oldest retained sample is evicted first.
    pub fn push(&mut self, level: f64) {
        if self.samples.len() == self.capacity {
            self.samples.pop_front();
        }
        self.samples.push_back(level);
    }

    /// Iterates retained samples oldest to newest.
    pub fn iter(&self) -> impl Iterator<Item = f64> + '_ {
        self.samples.iter().copied()
    }

    /// Number of samples currently retained (never exceeds [`Self::capacity`]).
    #[must_use]
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    /// True when no samples have been recorded yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// The maximum number of samples this history retains.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// The most recently pushed sample, or `None` if empty.
    #[must_use]
    pub fn latest(&self) -> Option<f64> {
        self.samples.back().copied()
    }
}

/// The direction a [`Flow`] acts on a [`Stock`]: an inflow adds
/// material, an outflow depletes it. Direction is a composition-time
/// concern — callers combine directed flow rates into the single
/// `net_flow` [`Stock::step`] expects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlowDirection {
    Inflow,
    Outflow,
}

/// A flow's rate, in one of the two STELLA/iThink pipe modes.
/// `Uniflow` mirrors a one-way pipe: negative rates clamp to zero.
/// `Biflow` mirrors a double-headed pipe: the rate may go negative.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Flow {
    Uniflow(f64),
    Biflow(f64),
}

impl Flow {
    #[must_use]
    pub fn rate(&self) -> f64 {
        match self {
            Flow::Uniflow(rate) => rate.max(0.0),
            Flow::Biflow(rate) => *rate,
        }
    }
}

/// An auxiliary, no-state algebraic element: holds a constant or
/// computes a function of other elements, recomputed every call to
/// [`Converter::value`] — never accumulated, unlike a [`Stock`].
pub struct Converter<F>
where
    F: Fn() -> f64,
{
    compute: F,
}

impl<F> Converter<F>
where
    F: Fn() -> f64,
{
    #[must_use]
    pub fn new(compute: F) -> Self {
        Self { compute }
    }

    #[must_use]
    pub fn value(&self) -> f64 {
        (self.compute)()
    }
}

/// An information-only link: a copied value snapshot, never a handle
/// back to the [`Stock`] or [`Converter`] it was read from. Structurally
/// carries information, never material — there is no method on
/// [`Connector`] that can mutate any [`Stock`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Connector {
    value: f64,
}

impl Connector {
    #[must_use]
    pub fn new(value: f64) -> Self {
        Self { value }
    }

    #[must_use]
    pub fn value(&self) -> f64 {
        self.value
    }
}

/// A model-boundary cloud terminal: represents state treated as
/// outside the model boundary (infinite-capacity source or sink).
/// Carries no state and is excluded from conservation checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Terminal {
    Source,
    Sink,
}

/// A causal loop's polarity: reinforcing (self-amplifying) or
/// balancing (goal-seeking).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopPolarity {
    Reinforcing,
    Balancing,
}

/// Classifies a causal loop's polarity from its count of negative
/// causal links, per adr-fmt-0pe95 sect 2: an even count (zero
/// counts as even) is reinforcing (R); an odd count is balancing (B).
#[must_use]
pub fn loop_polarity(negative_links: usize) -> LoopPolarity {
    if negative_links.is_multiple_of(2) {
        LoopPolarity::Reinforcing
    } else {
        LoopPolarity::Balancing
    }
}

/// Opaque handle to a [`Stock`] owned by a [`Model`]. Distinct from
/// [`FlowId`], [`ConverterId`], and [`CloudId`] at the type level so
/// the sealed endpoint-kind traits below can admit or reject a handle
/// per invariants 1, 3, 4, 5 of adr-fmt-qaavg without any runtime tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StockId(usize);

/// Opaque handle to a [`Flow`] edge owned by a [`Model`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FlowId(usize);

impl FlowId {
    /// Test-only constructor for exercising an out-of-range/unrouted
    /// [`FlowId`] against a [`Model`]/`Scene` that never registered
    /// it — every non-test caller derives a [`FlowId`] from
    /// [`Model::flows`] or a `Scene`'s already-routed belts.
    #[cfg(test)]
    pub(crate) fn from_raw(raw: usize) -> Self {
        Self(raw)
    }
}

/// Opaque handle to a [`Converter`] owned by a [`Model`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConverterId(usize);

/// Opaque handle to a [`Terminal`] (cloud) owned by a [`Model`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CloudId(usize);

mod sealed {
    pub trait Sealed {}
}

/// A flow's material endpoint identity, per adr-fmt-qaavg invariant 1:
/// a flow's two material endpoints are drawn from {Stock, Cloud}
/// exclusively.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlowTerminal {
    Stock(StockId),
    Cloud(CloudId),
}

/// A connector edge's node identity, per adr-fmt-qaavg invariants 3-6:
/// connector tails read Stock/Flow/Converter (never Cloud, invariant
/// 5); connector heads write into Flow/Converter only (never Stock —
/// invariant 3, never Cloud — invariant 6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectorNode {
    Stock(StockId),
    Flow(FlowId),
    Converter(ConverterId),
}

/// Sealed marker for types legal as a [`Flow`]'s material endpoint.
/// Implemented only for [`StockId`] and [`CloudId`] (adr-fmt-qaavg
/// invariant 1) — [`ConverterId`] and connector identities intentionally
/// have no impl, so passing one to [`Model::connect_flow`] fails to
/// type-check rather than failing at runtime (invariants 1 and 7).
pub trait FlowEndpoint: sealed::Sealed {
    #[doc(hidden)]
    fn into_flow_terminal(self) -> FlowTerminal;
}

impl sealed::Sealed for StockId {}
impl FlowEndpoint for StockId {
    fn into_flow_terminal(self) -> FlowTerminal {
        FlowTerminal::Stock(self)
    }
}

impl sealed::Sealed for CloudId {}
impl FlowEndpoint for CloudId {
    fn into_flow_terminal(self) -> FlowTerminal {
        FlowTerminal::Cloud(self)
    }
}

/// Sealed marker for types legal as a [`Connector`]'s tail (read
/// side). Implemented for [`StockId`], [`FlowId`], [`ConverterId`] —
/// deliberately NOT for [`CloudId`] (adr-fmt-qaavg invariant 5: clouds
/// carry no readable state).
pub trait ConnectorTail: sealed::Sealed {
    #[doc(hidden)]
    fn into_connector_node(self) -> ConnectorNode;
}

/// Sealed marker for types legal as a [`Connector`]'s head (write
/// side). Implemented only for [`FlowId`] and [`ConverterId`] —
/// deliberately NOT for [`StockId`] (adr-fmt-qaavg invariant 3: a
/// stock changes only via its attached flows) and NOT for [`CloudId`]
/// (invariant 6: clouds are never a connector endpoint).
pub trait ConnectorHead: sealed::Sealed {
    #[doc(hidden)]
    fn into_connector_node(self) -> ConnectorNode;
}

impl ConnectorTail for StockId {
    fn into_connector_node(self) -> ConnectorNode {
        ConnectorNode::Stock(self)
    }
}

impl ConnectorTail for FlowId {
    fn into_connector_node(self) -> ConnectorNode {
        ConnectorNode::Flow(self)
    }
}
impl sealed::Sealed for FlowId {}

impl ConnectorHead for FlowId {
    fn into_connector_node(self) -> ConnectorNode {
        ConnectorNode::Flow(self)
    }
}

impl ConnectorTail for ConverterId {
    fn into_connector_node(self) -> ConnectorNode {
        ConnectorNode::Converter(self)
    }
}
impl sealed::Sealed for ConverterId {}

impl ConnectorHead for ConverterId {
    fn into_connector_node(self) -> ConnectorNode {
        ConnectorNode::Converter(self)
    }
}

/// A stored material edge: an ordered pair of [`FlowTerminal`]s plus
/// the [`Flow`] rate mode connecting them.
struct FlowEdge {
    from: FlowTerminal,
    to: FlowTerminal,
    flow: Flow,
}

/// A stored information edge: an ordered pair of [`ConnectorNode`]s.
/// Carries no material value — only node identity — so it structurally
/// cannot move material (adr-fmt-qaavg invariant 2).
struct ConnectorEdge {
    #[expect(
        dead_code,
        reason = "stored for the invariant-8 advisory follow-up (stock-free loop detection); not yet consumed"
    )]
    tail: ConnectorNode,
    #[expect(
        dead_code,
        reason = "stored for the invariant-8 advisory follow-up (stock-free loop detection); not yet consumed"
    )]
    head: ConnectorNode,
}

/// Errors rejected only at [`Model::build`], i.e. rules that need
/// graph-global information a single [`Model::connect_flow`] or
/// [`Model::connect_info`] call cannot see. Marked `#[non_exhaustive]`
/// per CHE-0021:R1 / CHE-0094:R13 — new graph-global rules may add
/// variants without that being a breaking change.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SdConnectionError {
    /// Both of a flow's material endpoints are the same or different
    /// clouds: the edge has no effect on any tracked stock and is
    /// degenerate per the adr-fmt-qaavg connection matrix (cloud->cloud
    /// is listed forbidden as a no-op).
    DegenerateCloudToCloudFlow {
        /// Index of the offending flow edge, in insertion order.
        flow_index: usize,
    },
}

/// A system-dynamics model under construction: a non-generic builder
/// over [`StockId`], [`FlowId`], [`ConverterId`], [`CloudId`] handles.
/// Endpoint-kind legality for flows (invariants 1, 7) and connectors
/// (invariants 3, 4, 5, 6) is enforced at compile time via the sealed
/// [`FlowEndpoint`], [`ConnectorTail`], [`ConnectorHead`] traits — an
/// illegal edge simply does not type-check. Graph-global rules that a
/// single edge can't see (this iteration: cloud-to-cloud degenerate
/// flows) are checked once at [`Model::build`].
///
/// Invariant 8 of adr-fmt-qaavg (a genuine feedback loop passes
/// through at least one Stock) is ADVISORY ONLY this iteration: it is
/// the matrix's weakest-sourced/inferred claim, and enforcing it needs
/// cycle detection over the connector+flow graph, which this builder
/// does not implement. A model may legally construct a stock-free
/// algebraic loop (Converter/Connector cycle with no Stock); such a
/// loop is a known, intentionally-unrejected follow-up, not silently
/// assumed absent.
///
/// Converters are stored behind `Box<dyn Fn() -> f64>` rather than the
/// generic [`Converter<F>`] so `Model` itself carries no type
/// parameter — a deliberate design choice so this builder does not
/// infect `binding.rs` or `view.rs` public signatures with a generic
/// or lifetime parameter (mission `sd-ci-02-api` abort condition).
///
/// # Compile-fail evidence for the sealed endpoint-kind rules
///
/// Invariant 1 / 7 — a converter is never a flow's material endpoint:
///
/// ```compile_fail
/// let mut model = cherry_pit_sd_viz::sd::Model::new();
/// let stock = model.add_stock(cherry_pit_sd_viz::sd::Stock::new(0.0));
/// let converter = model.add_converter(|| 1.0);
/// model.connect_flow(converter, stock, cherry_pit_sd_viz::sd::Flow::Uniflow(1.0));
/// ```
///
/// Invariant 3 — a connector head is never a stock:
///
/// ```compile_fail
/// let mut model = cherry_pit_sd_viz::sd::Model::new();
/// let stock = model.add_stock(cherry_pit_sd_viz::sd::Stock::new(0.0));
/// let cloud = model.add_cloud(cherry_pit_sd_viz::sd::Terminal::Source);
/// let flow = model.connect_flow(cloud, stock, cherry_pit_sd_viz::sd::Flow::Uniflow(1.0));
/// model.connect_info(flow, stock);
/// ```
///
/// Invariant 4 / 6 — a connector head is never a cloud:
///
/// ```compile_fail
/// let mut model = cherry_pit_sd_viz::sd::Model::new();
/// let stock = model.add_stock(cherry_pit_sd_viz::sd::Stock::new(0.0));
/// let cloud = model.add_cloud(cherry_pit_sd_viz::sd::Terminal::Sink);
/// model.connect_info(stock, cloud);
/// ```
///
/// Invariant 5 / 6 — a connector tail is never a cloud:
///
/// ```compile_fail
/// let mut model = cherry_pit_sd_viz::sd::Model::new();
/// let stock = model.add_stock(cherry_pit_sd_viz::sd::Stock::new(0.0));
/// let cloud = model.add_cloud(cherry_pit_sd_viz::sd::Terminal::Source);
/// let flow = model.connect_flow(cloud, stock, cherry_pit_sd_viz::sd::Flow::Uniflow(1.0));
/// model.connect_info(cloud, flow);
/// ```
#[derive(Default)]
pub struct Model {
    stocks: Vec<Stock>,
    clouds: Vec<Terminal>,
    converters: Vec<Box<dyn Fn() -> f64>>,
    flows: Vec<FlowEdge>,
    connectors: Vec<ConnectorEdge>,
}

impl Model {
    /// Creates an empty model.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a [`Stock`], returning its handle.
    pub fn add_stock(&mut self, stock: Stock) -> StockId {
        self.stocks.push(stock);
        StockId(self.stocks.len() - 1)
    }

    /// Registers a [`Terminal`] (cloud), returning its handle.
    pub fn add_cloud(&mut self, cloud: Terminal) -> CloudId {
        self.clouds.push(cloud);
        CloudId(self.clouds.len() - 1)
    }

    /// Registers a converter's algebraic function, returning its
    /// handle. `compute` is boxed internally so `Model` stays
    /// non-generic regardless of how many distinct converter closures
    /// a caller registers.
    pub fn add_converter<F>(&mut self, compute: F) -> ConverterId
    where
        F: Fn() -> f64 + 'static,
    {
        self.converters.push(Box::new(compute));
        ConverterId(self.converters.len() - 1)
    }

    /// Connects a material [`Flow`] between two endpoints. Both `from`
    /// and `to` must implement [`FlowEndpoint`] — only [`StockId`] and
    /// [`CloudId`] do, per adr-fmt-qaavg invariant 1, so passing a
    /// [`ConverterId`] here does not compile (invariant 7 falls out
    /// for free: a converter can never sit in a flow's material path).
    pub fn connect_flow(
        &mut self,
        from: impl FlowEndpoint,
        to: impl FlowEndpoint,
        flow: Flow,
    ) -> FlowId {
        self.flows.push(FlowEdge {
            from: from.into_flow_terminal(),
            to: to.into_flow_terminal(),
            flow,
        });
        FlowId(self.flows.len() - 1)
    }

    /// Connects an information-only [`Connector`] edge from `tail`
    /// (read side) to `head` (write side). `tail` must implement
    /// [`ConnectorTail`] (Stock, Flow, or Converter — never Cloud, per
    /// invariant 5); `head` must implement [`ConnectorHead`] (Flow or
    /// Converter — never Stock, invariant 3/4; never Cloud, invariant
    /// 6). Illegal combinations do not compile. Stores only node
    /// identity, never a value or a handle capable of mutating a
    /// stock, so no material can move through the returned edge
    /// (invariant 2).
    pub fn connect_info(&mut self, tail: impl ConnectorTail, head: impl ConnectorHead) {
        self.connectors.push(ConnectorEdge {
            tail: tail.into_connector_node(),
            head: head.into_connector_node(),
        });
    }

    /// Finalizes the model, running the graph-global checks that a
    /// single [`Model::connect_flow`] call cannot see on its own.
    ///
    /// # Errors
    ///
    /// Returns [`SdConnectionError::DegenerateCloudToCloudFlow`] when
    /// any registered flow has both material endpoints as clouds — an
    /// edge with no effect on any tracked stock, forbidden as a no-op
    /// per the adr-fmt-qaavg connection matrix.
    pub fn build(self) -> Result<Self, SdConnectionError> {
        for (flow_index, edge) in self.flows.iter().enumerate() {
            if matches!(
                (&edge.from, &edge.to),
                (FlowTerminal::Cloud(_), FlowTerminal::Cloud(_))
            ) {
                return Err(SdConnectionError::DegenerateCloudToCloudFlow { flow_index });
            }
        }
        Ok(self)
    }

    /// Number of stocks registered so far.
    #[must_use]
    pub fn stock_count(&self) -> usize {
        self.stocks.len()
    }

    /// Number of flow edges registered so far.
    #[must_use]
    pub fn flow_count(&self) -> usize {
        self.flows.len()
    }

    /// Number of connector edges registered so far.
    #[must_use]
    pub fn connector_count(&self) -> usize {
        self.connectors.len()
    }

    /// Number of converters registered so far.
    #[must_use]
    pub fn converter_count(&self) -> usize {
        self.converters.len()
    }

    /// Number of cloud terminals registered so far.
    #[must_use]
    pub fn cloud_count(&self) -> usize {
        self.clouds.len()
    }

    /// Read-only view of every registered flow edge, in insertion
    /// order, as a [`FlowView`] — sufficient to derive a scene graph's
    /// belts without exposing [`FlowEdge`] internals.
    pub fn flows(&self) -> impl Iterator<Item = FlowView> + '_ {
        self.flows.iter().enumerate().map(|(index, edge)| FlowView {
            id: FlowId(index),
            tail: edge.from,
            head: edge.to,
            kind: edge.flow,
        })
    }

    /// Overwrites the rate of the [`Flow`] already registered at `id`,
    /// leaving its material endpoints untouched — the live-rate
    /// wiring path (adr-fmt-sra3p `svs-05`) a per-tick caller uses to
    /// replace a flow's placeholder rate with one derived from the
    /// running sim's measured per-tick activity. A no-op when `id`
    /// does not name a flow this model registered (defensive; every
    /// caller in this crate derives `id` from [`Self::flows`]).
    pub fn set_flow_rate(&mut self, id: FlowId, flow: Flow) {
        if let Some(edge) = self.flows.get_mut(id.0) {
            edge.flow = flow;
        }
    }

    /// Read-only iterator over every registered stock's handle, in
    /// insertion order, for keying a placement layer.
    pub fn stock_ids(&self) -> impl Iterator<Item = StockId> + '_ {
        (0..self.stocks.len()).map(StockId)
    }

    /// Read-only iterator over every registered cloud terminal's
    /// handle, in insertion order, for keying a placement layer.
    pub fn cloud_ids(&self) -> impl Iterator<Item = CloudId> + '_ {
        (0..self.clouds.len()).map(CloudId)
    }

    /// Read-only iterator over every registered converter's handle, in
    /// insertion order, for keying a placement layer.
    pub fn converter_ids(&self) -> impl Iterator<Item = ConverterId> + '_ {
        (0..self.converters.len()).map(ConverterId)
    }
}

/// A read-only, dependency-free view of one registered flow edge:
/// its [`FlowId`], its material tail and head [`FlowTerminal`]s, and
/// its rate [`Flow`] kind (Uniflow/Biflow) — everything a caller needs
/// to derive a scene graph's belts from a [`Model`] without depending
/// on the private [`FlowEdge`] representation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FlowView {
    pub id: FlowId,
    pub tail: FlowTerminal,
    pub head: FlowTerminal,
    pub kind: Flow,
}

#[cfg(test)]
mod tests {
    use super::{
        Connector, Converter, Flow, FlowTerminal, LevelHistory, LoopPolarity, Model,
        SdConnectionError, Stock, Terminal, loop_polarity,
    };

    #[test]
    fn tier1_model_flows_enumerate_all_eleven_with_stock_or_cloud_endpoints() {
        let model = crate::binding::tier1_model().expect("Tier-1 spine must build");
        let views: Vec<_> = model.flows().collect();
        assert_eq!(views.len(), 11, "Tier-1 spine has 11 material flow edges");
        for view in &views {
            assert!(
                matches!(view.tail, FlowTerminal::Stock(_) | FlowTerminal::Cloud(_)),
                "flow tail must be a Stock or Cloud terminal"
            );
            assert!(
                matches!(view.head, FlowTerminal::Stock(_) | FlowTerminal::Cloud(_)),
                "flow head must be a Stock or Cloud terminal"
            );
            assert!(
                matches!(view.kind, Flow::Uniflow(_) | Flow::Biflow(_)),
                "flow kind must be Uniflow or Biflow"
            );
        }
    }

    #[test]
    fn set_flow_rate_overwrites_only_the_targeted_flows_kind() {
        let mut model = Model::new();
        let cloud = model.add_cloud(Terminal::Source);
        let stock = model.add_stock(Stock::new(0.0));
        let other_stock = model.add_stock(Stock::new(0.0));
        let target = model.connect_flow(cloud, stock, Flow::Uniflow(0.0));
        let untouched = model.connect_flow(cloud, other_stock, Flow::Uniflow(2.0));

        model.set_flow_rate(target, Flow::Uniflow(9.0));

        let views: Vec<_> = model.flows().collect();
        assert_eq!(views[target.0].kind, Flow::Uniflow(9.0));
        assert_eq!(views[untouched.0].kind, Flow::Uniflow(2.0));
    }

    #[test]
    fn set_flow_rate_on_an_unregistered_id_is_a_no_op() {
        let mut model = Model::new();
        let cloud = model.add_cloud(Terminal::Source);
        let stock = model.add_stock(Stock::new(0.0));
        model.connect_flow(cloud, stock, Flow::Uniflow(1.0));
        let bogus = super::FlowId::from_raw(41);

        model.set_flow_rate(bogus, Flow::Uniflow(99.0));

        let views: Vec<_> = model.flows().collect();
        assert_eq!(views.len(), 1);
        assert_eq!(views[0].kind, Flow::Uniflow(1.0));
    }

    #[test]
    fn tier1_model_id_enumerations_match_registered_counts() {
        let model = crate::binding::tier1_model().expect("Tier-1 spine must build");
        assert_eq!(model.stock_ids().count(), model.stock_count());
        assert_eq!(model.cloud_ids().count(), model.cloud_count());
        assert_eq!(model.converter_ids().count(), model.converter_count());
    }

    #[test]
    fn euler_integration_under_constant_net_flow() {
        let mut stock = Stock::new(10.0);
        let dt = 0.5;
        let net_flow = 4.0;
        for _ in 0..4 {
            stock.step(dt, net_flow);
        }
        assert!(
            (stock.level() - 18.0).abs() < f64::EPSILON,
            "expected 10 + 4*(0.5*4) = 18, got {}",
            stock.level()
        );
    }

    #[test]
    fn uniflow_clamps_negative_rate_to_zero() {
        let flow = Flow::Uniflow(-3.0);
        assert!(flow.rate().abs() < f64::EPSILON);
        let flow = Flow::Uniflow(5.0);
        assert!((flow.rate() - 5.0).abs() < f64::EPSILON);
    }

    #[test]
    fn biflow_permits_negative_rate() {
        let flow = Flow::Biflow(-3.0);
        assert!((flow.rate() - (-3.0)).abs() < f64::EPSILON);
    }

    #[test]
    fn connector_carries_information_not_material() {
        let mut stock = Stock::new(100.0);
        let connector = Connector::new(stock.level());
        stock.step(1.0, -50.0);
        assert!(
            (connector.value() - 100.0).abs() < f64::EPSILON,
            "connector must be an unlinked snapshot, unaffected by a later stock mutation"
        );
        assert!(
            (stock.level() - 50.0).abs() < f64::EPSILON,
            "the stock mutation itself must still have taken effect"
        );
    }

    #[test]
    fn converter_recomputes_each_call_never_accumulates() {
        let stock = Stock::new(7.0);
        let doubled = Converter::new(|| stock.level() * 2.0);
        assert!((doubled.value() - 14.0).abs() < f64::EPSILON);
        assert!(
            (doubled.value() - 14.0).abs() < f64::EPSILON,
            "a converter recomputes; repeated calls with unchanged inputs give the same value"
        );
    }

    #[test]
    fn terminal_variants_are_source_and_sink() {
        assert_ne!(Terminal::Source, Terminal::Sink);
    }

    #[test]
    fn loop_polarity_even_negatives_is_reinforcing() {
        assert_eq!(loop_polarity(0), LoopPolarity::Reinforcing);
        assert_eq!(loop_polarity(2), LoopPolarity::Reinforcing);
    }

    #[test]
    fn loop_polarity_odd_negatives_is_balancing() {
        assert_eq!(loop_polarity(1), LoopPolarity::Balancing);
        assert_eq!(loop_polarity(3), LoopPolarity::Balancing);
    }

    #[test]
    fn level_history_empty_on_construction() {
        let history = LevelHistory::new(3);
        assert_eq!(history.len(), 0);
        assert!(history.is_empty());
        assert_eq!(history.capacity(), 3);
        assert_eq!(history.latest(), None);
        assert_eq!(history.iter().collect::<Vec<_>>(), Vec::<f64>::new());
    }

    #[test]
    fn level_history_capacity_zero_clamps_to_one() {
        let mut history = LevelHistory::new(0);
        assert_eq!(history.capacity(), 1);
        history.push(1.0);
        history.push(2.0);
        assert_eq!(history.len(), 1);
        assert_eq!(history.latest(), Some(2.0));
    }

    #[test]
    fn level_history_push_beyond_capacity_evicts_oldest() {
        let mut history = LevelHistory::new(3);
        history.push(1.0);
        history.push(2.0);
        history.push(3.0);
        history.push(4.0);
        assert_eq!(history.len(), 3);
        assert_eq!(history.iter().collect::<Vec<_>>(), vec![2.0, 3.0, 4.0]);
    }

    #[test]
    fn level_history_iter_order_is_oldest_to_newest() {
        let mut history = LevelHistory::new(5);
        for sample in [10.0, 20.0, 30.0] {
            history.push(sample);
        }
        assert_eq!(history.iter().collect::<Vec<_>>(), vec![10.0, 20.0, 30.0]);
    }

    #[test]
    fn level_history_len_bounded_by_capacity() {
        let mut history = LevelHistory::new(2);
        for sample in [1.0, 2.0, 3.0, 4.0, 5.0] {
            history.push(sample);
            assert!(history.len() <= history.capacity());
        }
        assert_eq!(history.len(), 2);
    }

    #[test]
    fn level_history_latest_tracks_most_recent_push() {
        let mut history = LevelHistory::new(4);
        assert_eq!(history.latest(), None);
        history.push(7.0);
        assert_eq!(history.latest(), Some(7.0));
        history.push(9.0);
        assert_eq!(history.latest(), Some(9.0));
    }

    #[test]
    fn flow_endpoint_stock_to_cloud_is_legal() {
        let mut model = Model::new();
        let stock = model.add_stock(Stock::new(50.0));
        let cloud = model.add_cloud(Terminal::Sink);
        model.connect_flow(stock, cloud, Flow::Uniflow(2.0));
        assert_eq!(model.flow_count(), 1);
    }

    #[test]
    fn connector_edge_stores_identity_not_material_value() {
        let mut model = Model::new();
        let stock = model.add_stock(Stock::new(100.0));
        let cloud = model.add_cloud(Terminal::Sink);
        let flow = model.connect_flow(stock, cloud, Flow::Uniflow(1.0));
        model.connect_info(stock, flow);
        assert_eq!(model.connector_count(), 1);
        assert!((model.stocks[0].level() - 100.0).abs() < f64::EPSILON);
    }

    #[test]
    fn connector_tail_stock_into_flow_is_legal() {
        let mut model = Model::new();
        let stock = model.add_stock(Stock::new(10.0));
        let cloud = model.add_cloud(Terminal::Sink);
        let flow = model.connect_flow(stock, cloud, Flow::Uniflow(1.0));
        model.connect_info(stock, flow);
        assert_eq!(model.connector_count(), 1);
    }

    #[test]
    fn connector_head_into_converter_is_legal() {
        let mut model = Model::new();
        let stock = model.add_stock(Stock::new(3.0));
        let converter = model.add_converter(|| 42.0);
        model.connect_info(stock, converter);
        assert_eq!(model.connector_count(), 1);
        assert_eq!(model.converter_count(), 1);
    }

    #[test]
    fn connector_tail_converter_into_flow_is_legal() {
        let mut model = Model::new();
        let stock = model.add_stock(Stock::new(0.0));
        let cloud = model.add_cloud(Terminal::Source);
        let converter = model.add_converter(|| 5.0);
        let flow = model.connect_flow(cloud, stock, Flow::Uniflow(1.0));
        model.connect_info(converter, flow);
        assert_eq!(model.connector_count(), 1);
    }

    #[test]
    fn flow_endpoint_cloud_to_stock_is_legal() {
        let mut model = Model::new();
        let stock = model.add_stock(Stock::new(0.0));
        let cloud = model.add_cloud(Terminal::Source);
        model.connect_flow(cloud, stock, Flow::Uniflow(4.0));
        assert_eq!(model.flow_count(), 1);
    }

    #[test]
    fn converter_feeds_flow_via_connector_is_legal() {
        let mut model = Model::new();
        let stock = model.add_stock(Stock::new(0.0));
        let cloud = model.add_cloud(Terminal::Source);
        let converter = model.add_converter(|| 9.0);
        let flow = model.connect_flow(cloud, stock, Flow::Uniflow(1.0));
        model.connect_info(converter, flow);
        assert_eq!(model.connector_count(), 1);
        assert_eq!(model.flow_count(), 1);
    }

    #[test]
    fn cloud_to_cloud_flow_builds_err_degenerate_cloud_to_cloud() {
        let mut model = Model::new();
        let source = model.add_cloud(Terminal::Source);
        let sink = model.add_cloud(Terminal::Sink);
        model.connect_flow(source, sink, Flow::Uniflow(1.0));
        assert!(matches!(
            model.build(),
            Err(SdConnectionError::DegenerateCloudToCloudFlow { flow_index: 0 })
        ));
    }

    #[test]
    fn stock_to_cloud_flow_builds_successfully() {
        let mut model = Model::new();
        let stock = model.add_stock(Stock::new(1.0));
        let cloud = model.add_cloud(Terminal::Sink);
        model.connect_flow(stock, cloud, Flow::Uniflow(1.0));
        assert!(model.build().is_ok());
    }

    #[test]
    fn cloud_count_tracks_registered_clouds() {
        let mut model = Model::new();
        assert_eq!(model.cloud_count(), 0);
        model.add_cloud(Terminal::Source);
        model.add_cloud(Terminal::Sink);
        assert_eq!(model.cloud_count(), 2);
    }
}
