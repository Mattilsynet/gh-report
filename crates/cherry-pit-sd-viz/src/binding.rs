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

use crate::sd::{Connector, Flow, LoopPolarity, Stock};
use crate::sim::{EnqueueResult, Sim, StepEvents};

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
}

impl QueueStockBinding {
    /// Seeds the macro Stock from the queue's current depth.
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
        }
    }

    /// Advances the macro Stock by one Euler step, deriving inflow from
    /// accepted arrivals in `events` and outflow from the depth delta the
    /// queue cannot otherwise account for (depth changes only via an
    /// accepted enqueue or a dequeue into a worker slot).
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
    }

    #[must_use]
    pub fn level(&self) -> f64 {
        self.stock.level()
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

#[cfg(test)]
mod tests {
    use super::QueueStockBinding;
    use crate::sd::{Flow, LoopPolarity};
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
}
