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

#[cfg(test)]
mod tests {
    use super::{
        Connector, Converter, Flow, LevelHistory, LoopPolarity, Stock, Terminal, loop_polarity,
    };

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
}
