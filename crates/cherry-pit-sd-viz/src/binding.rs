//! App-facing binding of gh-report's [`crate::sim::WorkQueue`] /
//! [`crate::sim::BatchTracker`] onto the generic STELLA SD core
//! ([`crate::sd`]), per adr-fmt-0pe95 sect 3-4 and CHE-0094:R7 — the
//! generic `sd` module stays app-agnostic; this submodule owns the
//! queue-specific mapping so no queue/gh-report type ever leaks into
//! `sd.rs`.
//!
//! Mapping (adr-fmt-0pe95 sect 3): `WorkQueue::depth()` <-> [`Stock`]
//! level; accepted arrivals <-> [`Flow::Uniflow`] inflow; jobs leaving the
//! queue for a worker slot <-> [`Flow::Uniflow`] outflow. Backpressure
//! (sect 4) is the canonical single balancing (B) loop: as the queue
//! depth rises toward `WorkQueue::capacity()`, a utilization
//! [`Connector`] documents the signal `WorkQueue::enqueue` already acts
//! on when it starts returning `EnqueueResult::QueueFull` — the loop
//! closing the gap toward the capacity ceiling.
//!
//! Little's Law (adr-fmt-0pe95 sect 3): `L = lambda * W`, rearranged
//! `W = L / lambda` — mean residence time is the macro Stock level
//! divided by the effective (accepted) arrival rate. See
//! [`QueueStockBinding::mean_residence_ticks`].

use crate::layout;
use crate::sd::{
    Connector, Flow, LevelHistory, LoopPolarity, Model, SdConnectionError, Stock, Terminal,
};
use crate::sim::{EnqueueResult, Sim, StepEvents};

/// Sample capacity for [`QueueStockBinding`]'s recorded level history:
/// 180 samples, matching a 1 sample/tick cadence at the "3 min at 1Hz"
/// end of the brief's stated 2-5 minute sparkline window
/// (`bootstrap.js`'s `setInterval` drives one [`Sim::step`] per tick).
const LEVEL_HISTORY_CAPACITY: usize = 180;

/// The macro SD view of [`crate::sim::WorkQueue`]: a [`Stock`] whose
/// level tracks `WorkQueue::depth()` exactly, advanced one Euler step
/// (`dt = 1`) per [`Sim::step`] tick via [`Self::advance`] — the same
/// running [`Sim`] the DES micro-view (`Sim::queue_depth`) drives.
pub struct QueueStockBinding {
    stock: Stock,
    capacity: f64,
    cumulative_accepted: u64,
    last_inflow: Flow,
    last_outflow: Flow,
    level_history: LevelHistory,
}

impl QueueStockBinding {
    /// Seeds the macro Stock from the queue's current depth. The level
    /// history starts empty (not seeded with this initial level): each
    /// [`Self::advance`] call records exactly one sample, so after `N`
    /// calls the history holds `min(N, capacity)` samples — the
    /// simplest, least-surprising accounting for a caller counting
    /// ticks against the history it observes.
    #[must_use]
    #[expect(
        clippy::cast_precision_loss,
        reason = "queue depth/capacity are bounded well under 2^52 in any realistic sim config"
    )]
    pub fn new(sim: &Sim) -> Self {
        Self {
            stock: Stock::new(sim.queue_depth() as f64),
            capacity: sim.queue_capacity() as f64,
            cumulative_accepted: 0,
            last_inflow: Flow::Uniflow(0.0),
            last_outflow: Flow::Uniflow(0.0),
            level_history: LevelHistory::new(LEVEL_HISTORY_CAPACITY),
        }
    }

    /// Advances the macro Stock by one Euler step, deriving inflow from
    /// accepted arrivals in `events` and outflow from the depth delta the
    /// queue cannot otherwise account for (depth changes only via an
    /// accepted enqueue or a dequeue into a worker slot). Records the
    /// post-step level into [`Self::level_history`].
    #[expect(
        clippy::cast_precision_loss,
        reason = "per-tick accepted counts and queue depth are bounded well under 2^52"
    )]
    pub fn advance(&mut self, events: &StepEvents, sim: &Sim) {
        let accepted = events
            .arrivals
            .iter()
            .filter(|(_, result)| *result == EnqueueResult::Accepted)
            .count();
        self.cumulative_accepted += accepted as u64;
        let before = self.stock.level();
        let new_depth = sim.queue_depth() as f64;
        let dequeued = (before + accepted as f64 - new_depth).max(0.0);
        self.last_inflow = Flow::Uniflow(accepted as f64);
        self.last_outflow = Flow::Uniflow(dequeued);
        let net_flow = self.last_inflow.rate() - self.last_outflow.rate();
        self.stock.step(1.0, net_flow);
        self.level_history.push(self.stock.level());
    }

    #[must_use]
    pub fn level(&self) -> f64 {
        self.stock.level()
    }

    /// Read-only access to the recorded "last N ticks" level history
    /// for sparkline rendering, oldest to newest.
    #[must_use]
    pub fn level_history(&self) -> &LevelHistory {
        &self.level_history
    }

    #[must_use]
    pub fn inflow(&self) -> Flow {
        self.last_inflow
    }

    #[must_use]
    pub fn outflow(&self) -> Flow {
        self.last_outflow
    }

    /// Utilization [`Connector`] (adr-fmt-0pe95 sect 4): the
    /// information-only snapshot the backpressure balancing loop reads
    /// to explain why `WorkQueue::enqueue` is about to start returning
    /// `EnqueueResult::QueueFull`.
    #[must_use]
    pub fn utilization(&self) -> Connector {
        let utilization = if self.capacity > 0.0 {
            self.stock.level() / self.capacity
        } else {
            0.0
        };
        Connector::new(utilization)
    }

    /// The backpressure loop's polarity: always Balancing (B) — a rising
    /// queue depth feeds back, via [`Self::utilization`], to throttle the
    /// effective inflow as capacity is approached (adr-fmt-0pe95 sect 4).
    #[must_use]
    pub fn backpressure_polarity(&self) -> LoopPolarity {
        LoopPolarity::Balancing
    }

    /// Little's Law (adr-fmt-0pe95 sect 3): mean residence time
    /// `W = L / lambda`, where `L` is the current Stock level and
    /// `lambda` is the cumulative effective (accepted) arrival rate per
    /// tick since this binding was created. `None` when no arrivals have
    /// yet been accepted (`lambda == 0`, residence time undefined).
    #[must_use]
    #[expect(
        clippy::cast_precision_loss,
        reason = "cumulative accepted count and elapsed ticks are bounded well under 2^52 for any sim run"
    )]
    pub fn mean_residence_ticks(&self, ticks_elapsed: u64) -> Option<f64> {
        if self.cumulative_accepted == 0 || ticks_elapsed == 0 {
            return None;
        }
        let lambda = self.cumulative_accepted as f64 / ticks_elapsed as f64;
        Some(self.stock.level() / lambda)
    }
}

/// A generic per-tick readout binding for any Tier-1 stock that has
/// only a bare level accessor to drive from (`in_flight`,
/// `BatchTracker` remaining, `EvidenceProjection`,
/// [`crate::layout::StockKind::Monotonic`] accumulators) — unlike
/// [`QueueStockBinding`], which has explicit accepted/dequeued counts
/// available from [`StepEvents`], these stocks only expose
/// `level() -> usize` each tick, so [`Self::advance`] derives
/// `(inflow, outflow)` from the raw level delta via
/// [`layout::level_delta_flows`] rather than a dedicated event count.
pub struct ReadoutStock {
    history: LevelHistory,
    last_level: f64,
}

impl ReadoutStock {
    /// Seeds from `initial_level`; the level history starts empty
    /// (mirrors [`QueueStockBinding::new`]'s convention — one sample
    /// per [`Self::advance`] call, not a pre-seeded first sample).
    #[must_use]
    pub fn new(initial_level: f64) -> Self {
        Self {
            history: LevelHistory::new(LEVEL_HISTORY_CAPACITY),
            last_level: initial_level,
        }
    }

    /// Advances to `new_level`, recording it into the history and
    /// returning the `(inflow, outflow)` pair
    /// [`layout::level_delta_flows`] derives from the delta since the
    /// last call.
    pub fn advance(&mut self, new_level: f64) -> (Flow, Flow) {
        let (inflow, outflow) = layout::level_delta_flows(self.last_level, new_level);
        self.last_level = new_level;
        self.history.push(new_level);
        (Flow::Uniflow(inflow), Flow::Uniflow(outflow))
    }

    #[must_use]
    pub fn level_history(&self) -> &LevelHistory {
        &self.history
    }
}

/// Builds the full gh-report Tier-1 SD spine (per adr-fmt-vrycy's
/// core-teaching-model ranking) as ONE legal [`Model`], wired through
/// the existing enforced grammar (adr-fmt-qaavg) with no new grammar
/// primitives and no new invariants. Per CHE-0094:R7, gh-report
/// vocabulary appears only as doc comments and local identifiers here
/// (`sd.rs` stays app-agnostic); this constructor carries no `Sim`
/// reference and no live values (structure only — live level readout
/// stays [`QueueStockBinding`]'s job, unchanged, once per tick).
///
/// Element inventory (adr-fmt-vrycy CORE TEACHING MODEL):
///
/// - `work_queue` [`Stock`] — [`crate::sim::Sim::queue_depth`].
/// - `in_flight` [`Stock`] — worker-pool WIP,
///   [`crate::sim::Sim::in_flight`].
/// - `batch_remaining` [`Stock`] — `BatchTracker` join barrier,
///   [`crate::sim::Sim::batch_remaining`].
/// - `evidence_projection` [`Stock`] — repositories captured,
///   [`crate::sim::Sim::repositories_captured`].
/// - `generation`, `served_pages`, `events_written` [`Stock`]s —
///   monotonic readout accumulators (inflow-only; adr-fmt-vrycy
///   ambiguity hotspot (d)), tracking
///   [`crate::sim::Sim::arcswap_generation`],
///   [`crate::sim::Sim::served_pages`], and
///   [`crate::sim::Sim::events_written`] respectively.
/// - `timer_source`, `github_source` [`Terminal::Source`] clouds —
///   the ScheduledBatch/External/InitialLoad boundary triggers.
/// - `github_sink`, `web_clients_sink`, `durable_sink`
///   [`Terminal::Sink`] clouds — collector-call consumption, served
///   pages/broadcasts, and the durable substrate (write-only per
///   adr-fmt-vrycy hotspot (a): nothing reads the durable count back,
///   so it is a Cloud, never a Stock, in this model).
/// - `utilization` [`crate::sd::Converter`] — reads `work_queue`,
///   feeds back into all three arrival flows via [`Model::connect_info`],
///   closing the B1 backpressure loop.
/// - `barrier_drained` converter — reads `batch_remaining`, gates
///   `finalize` via connector (adr-fmt-vrycy hotspot (c): the barrier
///   itself is Stock+Converter; the discrete `SweepPhase` label is not
///   an SD element and appears nowhere in this construction).
/// - `read_side` converter — reads `evidence_projection`'s LEVEL,
///   models `build_cached_pages`; feeds `finalize` via connector
///   (adr-fmt-vrycy hotspot (b): the read side is a converter chain,
///   not a second material stock-and-flow pipeline).
///
/// # Errors
///
/// Returns [`SdConnectionError`] if the wiring is graph-globally
/// illegal (e.g. a degenerate cloud-to-cloud flow) — expected to be
/// `Ok` for this construction; the `Err` path exists so callers (and
/// this module's own tests) can assert legality rather than assume it.
pub fn tier1_model() -> Result<Model, SdConnectionError> {
    let mut model = Model::new();

    let work_queue = model.add_stock(Stock::new(0.0));
    let in_flight = model.add_stock(Stock::new(0.0));
    let batch_remaining = model.add_stock(Stock::new(0.0));
    let evidence_projection = model.add_stock(Stock::new(0.0));
    let generation = model.add_stock(Stock::new(0.0));
    let served_pages = model.add_stock(Stock::new(0.0));
    let events_written = model.add_stock(Stock::new(0.0));

    let timer_source = model.add_cloud(Terminal::Source);
    let github_source = model.add_cloud(Terminal::Source);
    let github_sink = model.add_cloud(Terminal::Sink);
    let web_clients_sink = model.add_cloud(Terminal::Sink);
    let durable_sink = model.add_cloud(Terminal::Sink);

    let scheduled_batch = model.connect_flow(timer_source, work_queue, Flow::Uniflow(0.0));
    let external = model.connect_flow(github_source, work_queue, Flow::Uniflow(0.0));
    let initial_load = model.connect_flow(timer_source, work_queue, Flow::Uniflow(0.0));
    model.connect_flow(work_queue, in_flight, Flow::Uniflow(0.0));
    model.connect_flow(in_flight, evidence_projection, Flow::Uniflow(0.0));
    model.connect_flow(in_flight, github_sink, Flow::Uniflow(0.0));
    let finalize = model.connect_flow(timer_source, generation, Flow::Uniflow(0.0));
    model.connect_flow(generation, served_pages, Flow::Uniflow(0.0));
    model.connect_flow(served_pages, web_clients_sink, Flow::Uniflow(0.0));
    model.connect_flow(evidence_projection, durable_sink, Flow::Uniflow(0.0));
    model.connect_flow(evidence_projection, events_written, Flow::Uniflow(0.0));

    let utilization = model.add_converter(|| 0.0);
    let barrier_drained = model.add_converter(|| 0.0);
    let read_side = model.add_converter(|| 0.0);

    model.connect_info(work_queue, utilization);
    model.connect_info(utilization, scheduled_batch);
    model.connect_info(utilization, external);
    model.connect_info(utilization, initial_load);
    model.connect_info(batch_remaining, barrier_drained);
    model.connect_info(barrier_drained, finalize);
    model.connect_info(evidence_projection, read_side);
    model.connect_info(read_side, finalize);

    model.build()
}

/// The Tier-1 spine's single feedback loop (adr-fmt-vrycy CORE
/// TEACHING MODEL, B1): `work_queue` depth feeds [`Connector`] into
/// `utilization`, which feeds back into the arrival flows, throttling
/// effective inflow as capacity is approached — always Balancing,
/// mirroring [`QueueStockBinding::backpressure_polarity`].
#[must_use]
pub fn tier1_backpressure_polarity() -> LoopPolarity {
    LoopPolarity::Balancing
}

#[cfg(test)]
mod tests {
    use super::QueueStockBinding;
    use crate::sd::{Flow, LoopPolarity, Model, Stock, Terminal};
    use crate::sim::{Sim, SimConfig};

    fn config() -> SimConfig {
        SimConfig {
            queue_capacity: 8,
            worker_count: 4,
            service_ticks: 3,
            domain_key_span: 500,
            ..SimConfig::default()
        }
    }

    #[test]
    fn queue_model_wiring_builds_ok_under_enforced_grammar() {
        let mut model = Model::new();
        let stock = model.add_stock(Stock::new(0.0));
        let source = model.add_cloud(Terminal::Source);
        let sink = model.add_cloud(Terminal::Sink);
        model.connect_flow(source, stock, Flow::Uniflow(0.0));
        model.connect_flow(stock, sink, Flow::Uniflow(0.0));
        let utilization_aux = model.add_converter(|| 0.0);
        model.connect_info(stock, utilization_aux);
        model
            .build()
            .expect("queue model wiring must be legal under the enforced SD grammar");
    }

    #[test]
    #[expect(
        clippy::cast_precision_loss,
        reason = "test-only micro depth comparison; depth is bounded well under 2^52"
    )]
    fn macro_stock_agrees_with_micro_depth_at_checkpoint() {
        let mut sim = Sim::new(config(), 1234);
        let mut binding = QueueStockBinding::new(&sim);
        for tick in 0..50u64 {
            let events = sim.step(tick % 2 == 0, tick % 3 == 0);
            binding.advance(&events, &sim);
            assert!(
                (binding.level() - sim.queue_depth() as f64).abs() < f64::EPSILON,
                "macro Stock level {} disagreed with micro depth {} at tick {tick}",
                binding.level(),
                sim.queue_depth()
            );
        }
    }

    #[test]
    fn utilization_rises_toward_one_as_depth_nears_capacity() {
        let mut sim = Sim::new(
            SimConfig {
                queue_capacity: 2,
                worker_count: 0,
                service_ticks: 1,
                domain_key_span: 500,
                ..SimConfig::default()
            },
            7,
        );
        let mut binding = QueueStockBinding::new(&sim);
        let events = sim.step(true, false);
        binding.advance(&events, &sim);
        let events = sim.step(true, false);
        binding.advance(&events, &sim);
        assert!(
            (binding.utilization().value() - 1.0).abs() < f64::EPSILON,
            "expected full utilization at capacity, got {}",
            binding.utilization().value()
        );
    }

    #[test]
    fn backpressure_loop_is_balancing() {
        let sim = Sim::new(config(), 1);
        let binding = QueueStockBinding::new(&sim);
        assert_eq!(binding.backpressure_polarity(), LoopPolarity::Balancing);
    }

    #[test]
    fn mean_residence_none_before_any_arrival_accepted() {
        let sim = Sim::new(config(), 1);
        let binding = QueueStockBinding::new(&sim);
        assert_eq!(binding.mean_residence_ticks(10), None);
    }

    #[test]
    fn inflow_and_outflow_are_uniflow_direction() {
        let mut sim = Sim::new(config(), 42);
        let mut binding = QueueStockBinding::new(&sim);
        let events = sim.step(true, false);
        binding.advance(&events, &sim);
        assert!(matches!(binding.inflow(), Flow::Uniflow(_)));
        assert!(matches!(binding.outflow(), Flow::Uniflow(_)));
    }

    #[test]
    fn level_history_starts_empty() {
        let sim = Sim::new(config(), 1);
        let binding = QueueStockBinding::new(&sim);
        assert_eq!(binding.level_history().len(), 0);
        assert_eq!(binding.level_history().latest(), None);
    }

    #[test]
    fn level_history_holds_min_of_ticks_and_capacity_ending_at_current_level() {
        let mut sim = Sim::new(config(), 5);
        let mut binding = QueueStockBinding::new(&sim);
        for tick in 0..20u64 {
            let events = sim.step(tick % 2 == 0, tick % 3 == 0);
            binding.advance(&events, &sim);
        }
        assert_eq!(binding.level_history().len(), 20);
        assert!(
            (binding
                .level_history()
                .latest()
                .expect("20 samples recorded")
                - binding.level())
            .abs()
                < f64::EPSILON
        );
    }

    #[test]
    fn level_history_len_bounded_by_capacity_over_many_ticks() {
        let mut sim = Sim::new(config(), 6);
        let mut binding = QueueStockBinding::new(&sim);
        let capacity = binding.level_history().capacity();
        for tick in 0..(capacity as u64 + 50) {
            let events = sim.step(tick % 2 == 0, tick % 3 == 0);
            binding.advance(&events, &sim);
        }
        assert_eq!(binding.level_history().len(), capacity);
        assert!(
            (binding.level_history().latest().expect("samples recorded") - binding.level()).abs()
                < f64::EPSILON
        );
    }

    #[test]
    fn tier1_model_builds_ok_under_enforced_grammar() {
        assert!(
            super::tier1_model().is_ok(),
            "Tier-1 gh-report spine must be legal under the adr-fmt-qaavg enforced grammar"
        );
    }

    #[test]
    fn tier1_model_element_counts_match_core_teaching_model_inventory() {
        let model = super::tier1_model().expect("Tier-1 spine must build");
        assert_eq!(
            model.stock_count(),
            7,
            "work_queue, in_flight, batch_remaining, evidence_projection, generation, served_pages, events_written"
        );
        assert_eq!(
            model.cloud_count(),
            5,
            "timer_source, github_source, github_sink, web_clients_sink, durable_sink"
        );
        assert_eq!(
            model.converter_count(),
            3,
            "utilization, barrier_drained, read_side"
        );
        assert_eq!(
            model.flow_count(),
            11,
            "3 arrivals + dequeue + completion + github-consume + finalize + serve-counter + broadcast + durable-append + events-written"
        );
        assert_eq!(
            model.connector_count(),
            8,
            "work_queue->utilization, utilization->three arrivals, batch_remaining->barrier_drained->finalize, evidence_projection->read_side->finalize"
        );
    }

    #[test]
    fn tier1_model_element_count_invariant_structurally_excludes_sweep_phase() {
        let model = super::tier1_model().expect("Tier-1 spine must build");
        assert_eq!(
            (
                model.stock_count(),
                model.cloud_count(),
                model.converter_count(),
                model.flow_count(),
                model.connector_count()
            ),
            (7, 5, 3, 11, 8),
            "SweepPhase is control-flow (adr-fmt-vrycy hotspot (c)), never an sd::Model node; \
             any node representing it would perturb this exact tuple, which this constructor \
             never does — no add_stock/add_converter call anywhere maps to SweepPhase"
        );
    }

    #[test]
    fn tier1_backpressure_loop_is_balancing() {
        assert_eq!(
            super::tier1_backpressure_polarity(),
            LoopPolarity::Balancing
        );
    }

    #[test]
    fn readout_stock_advance_rising_level_reports_inflow_only() {
        let mut readout = super::ReadoutStock::new(0.0);
        let (inflow, outflow) = readout.advance(5.0);
        assert!((inflow.rate() - 5.0).abs() < f64::EPSILON);
        assert!((outflow.rate() - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn readout_stock_advance_falling_level_reports_outflow_only() {
        let mut readout = super::ReadoutStock::new(10.0);
        let (inflow, outflow) = readout.advance(3.0);
        assert!((inflow.rate() - 0.0).abs() < f64::EPSILON);
        assert!((outflow.rate() - 7.0).abs() < f64::EPSILON);
    }

    #[test]
    fn readout_stock_history_records_one_sample_per_advance() {
        let mut readout = super::ReadoutStock::new(0.0);
        readout.advance(1.0);
        readout.advance(2.0);
        readout.advance(3.0);
        assert_eq!(readout.level_history().len(), 3);
        assert_eq!(readout.level_history().latest(), Some(3.0));
    }
}
