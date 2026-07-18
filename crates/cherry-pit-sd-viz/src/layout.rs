//! Host-pure layout/format math shared by the SD component-template
//! family (`components.rs`, C1/C7): dot positioning, rate/percent/
//! residence formatting, and the [`StockKind`] suppression rules that
//! decide which parts of a generic stock box render for a monotonic
//! readout accumulator (`generation`/`served_pages`/`events_written`,
//! adr-fmt-vrycy ambiguity hotspot (d)) versus a standard stock with
//! outflow, dots, and residence time.
//!
//! Kept out of `sd.rs` (generic SD core stays free of rendering-domain
//! concerns) and out of the `wasm32`-gated `components.rs` (this math
//! is host-testable without a `wasm32` target), mirroring
//! [`crate::sparkline`]'s split.

use crate::sd::LoopPolarity;

/// Distinguishes a standard stock (dots, outflow, residence time all
/// meaningful — e.g. `WorkQueue`, `in_flight`, `BatchTracker`,
/// `EvidenceProjection`) from a monotonic readout accumulator whose
/// outflow is always `0` and whose residence time is undefined
/// (`generation`, `served_pages`, `events_written`; adr-fmt-vrycy
/// hotspot (d)). Two kinds only — this is a rendering-suppression
/// switch, not a modelling taxonomy; see [`crate::sd::Stock`] for the
/// single underlying SD type both kinds wrap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StockKind {
    Standard,
    Monotonic,
}

impl StockKind {
    /// Whether this kind's stock box renders live "now" dots (one per
    /// in-flight unit). `Monotonic` stocks have no queued units to
    /// dot — they only ever grow.
    #[must_use]
    pub fn shows_dots(self) -> bool {
        matches!(self, StockKind::Standard)
    }

    /// Whether this kind's stock box renders an outflow readout.
    /// `Monotonic` stocks are inflow-only by definition (adr-fmt-vrycy
    /// hotspot (d)) — an outflow field would always read `0.0`.
    #[must_use]
    pub fn shows_outflow(self) -> bool {
        matches!(self, StockKind::Standard)
    }

    /// Whether this kind's stock box renders a Little's-Law mean
    /// residence time. Residence time is undefined for a stock with no
    /// outflow (nothing ever leaves to measure a residence against).
    #[must_use]
    pub fn shows_residence(self) -> bool {
        matches!(self, StockKind::Standard)
    }
}

/// The x-coordinate of the `index`-th of `count` evenly-spaced "now"
/// dots across a `width`-wide viewBox, each dot centred in its slice
/// (`index + 0.5`, matching [`crate::sparkline::polyline_points`]'s
/// even-spacing convention).
#[must_use]
#[expect(
    clippy::cast_precision_loss,
    reason = "in-queue/in-flight counts are bounded well under 2^52 for any realistic capacity"
)]
pub fn dot_x(index: usize, count: usize, width: f64) -> f64 {
    if count == 0 {
        return 0.0;
    }
    width * (index as f64 + 0.5) / count as f64
}

/// Formats a flow rate to one decimal place, the shared display
/// convention for every inflow/outflow readout across the template
/// family.
#[must_use]
pub fn format_rate(rate: f64) -> String {
    format!("{rate:.1}")
}

/// Formats a `0.0..=1.0` fraction as a whole-percent readout (e.g.
/// utilization, compression ratio).
#[must_use]
pub fn format_percent(fraction: f64) -> String {
    format!("{:.0}%", fraction * 100.0)
}

/// Formats a Little's-Law mean residence time, or `"n/a"` when
/// undefined (no arrivals yet accepted, or a [`StockKind::Monotonic`]
/// stock, which suppresses this field entirely rather than calling
/// this function).
#[must_use]
pub fn format_residence(ticks: Option<f64>) -> String {
    ticks.map_or_else(|| "n/a".to_string(), |value| format!("{value:.1}"))
}

/// Formats a bounded-capacity level readout as `current/capacity`
/// (e.g. `WorkQueue` depth/capacity, `ClientPool` permits/max).
#[must_use]
pub fn format_bounded_level(current: usize, capacity: usize) -> String {
    format!("{current}/{capacity}")
}

/// The single-character badge label for a causal loop's polarity:
/// `"B"` (Balancing) or `"R"` (Reinforcing). adr-fmt-vrycy found 0 R
/// loops in gh-report's boundary (pure work-shedding/backpressure
/// system) — the `R` arm exists for completeness per oracle guidance,
/// not because any in-model loop currently uses it.
#[must_use]
pub fn polarity_badge_label(polarity: LoopPolarity) -> &'static str {
    match polarity {
        LoopPolarity::Balancing => "B",
        LoopPolarity::Reinforcing => "R",
    }
}

/// A Cloud terminal's boundary-direction glyph: `"->"` for a
/// [`crate::sd::Terminal::Source`] (material enters the model
/// boundary here), `"<-"` for a [`crate::sd::Terminal::Sink`]
/// (material leaves the model boundary here).
#[must_use]
pub fn cloud_direction_glyph(terminal: crate::sd::Terminal) -> &'static str {
    match terminal {
        crate::sd::Terminal::Source => "->",
        crate::sd::Terminal::Sink => "<-",
    }
}

/// The compression ratio readout (`compressed / raw` as a whole
/// percent), or `None` when `raw_bytes` is `0` (nothing compressed
/// yet — division would be meaningless, not a `0%` ratio).
#[must_use]
#[expect(
    clippy::cast_precision_loss,
    reason = "cumulative byte totals are bounded well under 2^52 for any realistic sim run"
)]
pub fn compression_ratio_percent(raw_bytes: usize, compressed_bytes: usize) -> Option<f64> {
    if raw_bytes == 0 {
        return None;
    }
    Some(compressed_bytes as f64 / raw_bytes as f64 * 100.0)
}

#[cfg(test)]
mod tests {
    use super::{
        StockKind, cloud_direction_glyph, compression_ratio_percent, dot_x, format_bounded_level,
        format_percent, format_rate, format_residence, polarity_badge_label,
    };
    use crate::sd::{LoopPolarity, Terminal};

    #[test]
    fn standard_kind_shows_dots_outflow_and_residence() {
        assert!(StockKind::Standard.shows_dots());
        assert!(StockKind::Standard.shows_outflow());
        assert!(StockKind::Standard.shows_residence());
    }

    #[test]
    fn monotonic_kind_suppresses_dots_outflow_and_residence() {
        assert!(!StockKind::Monotonic.shows_dots());
        assert!(!StockKind::Monotonic.shows_outflow());
        assert!(!StockKind::Monotonic.shows_residence());
    }

    #[test]
    fn dot_x_centres_each_slice() {
        assert!((dot_x(0, 2, 200.0) - 50.0).abs() < f64::EPSILON);
        assert!((dot_x(1, 2, 200.0) - 150.0).abs() < f64::EPSILON);
    }

    #[test]
    fn dot_x_zero_count_is_zero_not_nan() {
        assert!((dot_x(0, 0, 200.0) - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn format_rate_keeps_one_decimal() {
        assert_eq!(format_rate(3.0), "3.0");
        assert_eq!(format_rate(3.14159), "3.1");
    }

    #[test]
    fn format_percent_rounds_to_whole() {
        assert_eq!(format_percent(0.5), "50%");
        assert_eq!(format_percent(1.0), "100%");
    }

    #[test]
    fn format_residence_none_is_na() {
        assert_eq!(format_residence(None), "n/a");
    }

    #[test]
    fn format_residence_some_keeps_one_decimal() {
        assert_eq!(format_residence(Some(4.25)), "4.2");
    }

    #[test]
    fn format_bounded_level_is_current_slash_capacity() {
        assert_eq!(format_bounded_level(3, 8), "3/8");
    }

    #[test]
    fn polarity_badge_labels_match_polarity() {
        assert_eq!(polarity_badge_label(LoopPolarity::Balancing), "B");
        assert_eq!(polarity_badge_label(LoopPolarity::Reinforcing), "R");
    }

    #[test]
    fn cloud_direction_glyph_matches_terminal() {
        assert_eq!(cloud_direction_glyph(Terminal::Source), "->");
        assert_eq!(cloud_direction_glyph(Terminal::Sink), "<-");
    }

    #[test]
    fn compression_ratio_none_when_no_raw_bytes() {
        assert_eq!(compression_ratio_percent(0, 0), None);
    }

    #[test]
    fn compression_ratio_some_when_raw_bytes_present() {
        let ratio = compression_ratio_percent(100, 40).expect("raw bytes present");
        assert!((ratio - 40.0).abs() < f64::EPSILON);
    }
}
