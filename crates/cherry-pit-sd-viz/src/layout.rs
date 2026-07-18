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

/// Distinguishes four stock-box rendering shapes, decoupled per-field
/// (dots / outflow / utilization / residence) rather than one
/// all-or-nothing switch, so each Tier-1 stock (adr-fmt-vrycy CORE
/// TEACHING MODEL) gets only the fields it has real data for:
///
/// - [`StockKind::Standard`] — `WorkQueue`: capacity-bounded, live
///   "now" dots, outflow, utilization, and Little's-Law residence all
///   meaningful.
/// - [`StockKind::Bounded`] — `in_flight` (worker-pool WIP,
///   0..`worker_count`): outflow and utilization
///   (`in_flight`/`worker_count`) meaningful; no per-job dots layer
///   wired, no residence tracking (no cumulative-arrivals counter for
///   this stock).
/// - [`StockKind::Accumulator`] — `BatchTracker` remaining,
///   `EvidenceProjection`: real inflow AND outflow (both legitimately
///   non-zero — these are not monotonic), but no capacity ceiling to
///   express as utilization and no dots/residence data.
/// - [`StockKind::Monotonic`] — readout accumulator whose outflow is
///   always `0` and whose residence time is undefined (`generation`,
///   `served_pages`, `events_written`; adr-fmt-vrycy hotspot (d)).
///
/// This is a rendering-suppression switch, not a modelling taxonomy —
/// see [`crate::sd::Stock`] for the single underlying SD type all four
/// kinds wrap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StockKind {
    Standard,
    Bounded,
    Accumulator,
    Monotonic,
}

impl StockKind {
    /// Whether this kind's stock box renders live "now" dots (one per
    /// in-flight unit). Only [`StockKind::Standard`] (`WorkQueue`) has
    /// a caller-supplied per-unit color list to dot.
    #[must_use]
    pub fn shows_dots(self) -> bool {
        matches!(self, StockKind::Standard)
    }

    /// Whether this kind's stock box renders an outflow readout.
    /// [`StockKind::Monotonic`] stocks are inflow-only by definition
    /// (adr-fmt-vrycy hotspot (d)) — an outflow field would always
    /// read `0.0`. Every other kind has a real, sometimes-nonzero
    /// outflow.
    #[must_use]
    pub fn shows_outflow(self) -> bool {
        !matches!(self, StockKind::Monotonic)
    }

    /// Whether this kind's stock box renders a utilization readout.
    /// Only kinds with a meaningful capacity ceiling
    /// ([`StockKind::Standard`]'s queue capacity,
    /// [`StockKind::Bounded`]'s worker count) have one;
    /// [`StockKind::Accumulator`] stocks (`BatchTracker`,
    /// `EvidenceProjection`) have no capacity concept to divide by.
    #[must_use]
    pub fn shows_utilization(self) -> bool {
        matches!(self, StockKind::Standard | StockKind::Bounded)
    }

    /// Whether this kind's stock box renders a Little's-Law mean
    /// residence time. Only [`StockKind::Standard`] (`WorkQueue`)
    /// tracks the cumulative-accepted-arrivals counter Little's Law
    /// needs; other kinds would show a residence figure with no
    /// backing arrival-rate data.
    #[must_use]
    pub fn shows_residence(self) -> bool {
        matches!(self, StockKind::Standard)
    }

    /// Whether this kind's stock box renders a loop-polarity badge.
    /// Only [`StockKind::Standard`] (`WorkQueue`) is the stock the B1
    /// backpressure loop (adr-fmt-vrycy CORE TEACHING MODEL) reads.
    #[must_use]
    pub fn shows_polarity(self) -> bool {
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

/// Splits a raw level delta (`current - previous`) into an
/// `(inflow, outflow)` non-negative pair — the same bookkeeping
/// [`crate::binding::QueueStockBinding::advance`] does with explicit
/// accepted/dequeued counts, generalized to any stock whose per-tick
/// level readout is all a caller has (`in_flight`, `BatchTracker`
/// remaining, `EvidenceProjection`, and the monotonic accumulators,
/// which always take the `outflow == 0.0` branch since their levels
/// never fall).
#[must_use]
pub fn level_delta_flows(previous: f64, current: f64) -> (f64, f64) {
    let delta = current - previous;
    if delta >= 0.0 {
        (delta, 0.0)
    } else {
        (0.0, -delta)
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

/// Fixed spacing/sizing parameters for the computed grid layout
/// (adr-fmt-izwyo): margin from the viewBox edge, per-column and
/// per-row pitch (box size plus gutter), and the box's own
/// dimensions. One set of params drives [`grid_slot_origin`],
/// [`slot_anchor`], and [`grid_dimensions`] so node placement and
/// viewBox sizing can never drift apart.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridParams {
    pub margin: f64,
    pub col_pitch: f64,
    pub row_pitch: f64,
    pub box_width: f64,
    pub box_height: f64,
}

/// Which edge midpoint of a slot's box an edge-path anchor attaches
/// to: [`Side::Top`]/[`Side::Bottom`] for vertical flow (sources
/// above, clients below), [`Side::Left`]/[`Side::Right`] for
/// same-row or backward-referencing edges (e.g. `github_pull`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Top,
    Bottom,
    Left,
    Right,
}

/// The top-left `(x, y)` px of the box at zero-indexed `(row, col)`,
/// packing columns left-to-right and rows top-to-bottom so row 0
/// (sources) sits above every later row and the highest-`col` box of
/// the last row sits at the maximum x (adr-fmt-izwyo: sources
/// top-left, clients bottom-right).
#[must_use]
#[expect(
    clippy::cast_precision_loss,
    reason = "grid row/col indices are bounded well under 2^52 (row plan caps at 6 cols)"
)]
pub fn grid_slot_origin(row: usize, col: usize, params: GridParams) -> (f64, f64) {
    let x = params.margin + col as f64 * params.col_pitch;
    let y = params.margin + row as f64 * params.row_pitch;
    (x, y)
}

/// The px anchor point on the given [`Side`] of the box at
/// `(row, col)`, for edge-path routing: [`Side::Top`]/
/// [`Side::Bottom`] return the horizontal midpoint of that edge,
/// [`Side::Left`]/[`Side::Right`] the vertical midpoint.
#[must_use]
pub fn slot_anchor(row: usize, col: usize, side: Side, params: GridParams) -> (f64, f64) {
    let (x, y) = grid_slot_origin(row, col, params);
    let half_w = params.box_width / 2.0;
    let half_h = params.box_height / 2.0;
    match side {
        Side::Top => (x + half_w, y),
        Side::Bottom => (x + half_w, y + params.box_height),
        Side::Left => (x, y + half_h),
        Side::Right => (x + params.box_width, y + half_h),
    }
}

/// Builds an SVG cubic-bezier `d` string from one anchor to another,
/// bowing the curve through control points offset a third of the way
/// along the travel's dominant axis and held flat on the other —
/// vertical edges (`|dy| >= |dx|`, the common source-to-client flow)
/// bow via the x control offset, horizontal edges (`|dx| > |dy|`,
/// e.g. `github_pull`, `backend_pgno`/`backend_nats`, `clients_http`)
/// bow via the y control offset instead, so a horizontal edge doesn't
/// degenerate to a near-straight line. Same smooth-`C`-curve style as
/// the hand-authored `PATH_*` consts in `view.rs`. Deterministic: the
/// same `(from, to)` pair always yields the same string.
#[must_use]
pub fn bezier_edge_path(from: (f64, f64), to: (f64, f64)) -> String {
    let (fx, fy) = from;
    let (tx, ty) = to;
    let (dx, dy) = (tx - fx, ty - fy);
    let ((c1x, c1y), (c2x, c2y)) = if dy.abs() >= dx.abs() {
        ((fx + dx / 3.0, fy), (fx + dx * 2.0 / 3.0, ty))
    } else {
        ((fx, fy + dy / 3.0), (tx, fy + dy * 2.0 / 3.0))
    };
    format!("M{fx},{fy} C{c1x},{c1y} {c2x},{c2y} {tx},{ty}")
}

/// The total `(width, height)` in px a grid of `max_rows` by
/// `max_cols` boxes needs, so `view.rs`'s SVG `viewBox` and
/// `index.html`'s `.graph-canvas` can both be sized from the same
/// formula (adr-fmt-izwyo 1:1 requirement).
#[must_use]
#[expect(
    clippy::cast_precision_loss,
    reason = "grid row/col counts are bounded well under 2^52 (row plan caps at 6 cols, 5 rows)"
)]
pub fn grid_dimensions(max_rows: usize, max_cols: usize, params: GridParams) -> (f64, f64) {
    let width = 2.0f64.mul_add(
        params.margin,
        (max_cols as f64 - 1.0).mul_add(params.col_pitch, params.box_width),
    );
    let height = 2.0f64.mul_add(
        params.margin,
        (max_rows as f64 - 1.0).mul_add(params.row_pitch, params.box_height),
    );
    (width, height)
}

#[cfg(test)]
mod tests {
    use super::{
        GridParams, Side, StockKind, bezier_edge_path, cloud_direction_glyph,
        compression_ratio_percent, dot_x, format_bounded_level, format_percent, format_rate,
        format_residence, grid_dimensions, grid_slot_origin, level_delta_flows,
        polarity_badge_label, slot_anchor,
    };
    use crate::sd::{LoopPolarity, Terminal};

    const ROW_PLAN: [usize; 5] = [4, 5, 6, 5, 3];

    const GRID: GridParams = GridParams {
        margin: 40.0,
        col_pitch: 205.0,
        row_pitch: 200.0,
        box_width: 180.0,
        box_height: 92.0,
    };

    fn all_slots() -> Vec<(usize, usize)> {
        ROW_PLAN
            .iter()
            .enumerate()
            .flat_map(|(row, &cols)| (0..cols).map(move |col| (row, col)))
            .collect()
    }

    #[test]
    fn no_row_exceeds_six_columns() {
        assert!(ROW_PLAN.iter().all(|&cols| cols <= 6));
    }

    #[test]
    fn no_two_slots_overlap() {
        let slots = all_slots();
        for (i, &(row_a, col_a)) in slots.iter().enumerate() {
            let (xa, ya) = grid_slot_origin(row_a, col_a, GRID);
            for &(row_b, col_b) in &slots[i + 1..] {
                let (xb, yb) = grid_slot_origin(row_b, col_b, GRID);
                let x_overlap = xa < xb + GRID.box_width && xb < xa + GRID.box_width;
                let y_overlap = ya < yb + GRID.box_height && yb < ya + GRID.box_height;
                assert!(
                    !(x_overlap && y_overlap),
                    "slots ({row_a},{col_a}) and ({row_b},{col_b}) overlap"
                );
            }
        }
    }

    #[test]
    fn source_row_is_topmost_and_client_row_is_bottommost_rightmost() {
        let (_, source_y) = grid_slot_origin(0, 0, GRID);
        let last_row = ROW_PLAN.len() - 1;
        let last_row_cols = ROW_PLAN[last_row];

        for (row, &cols) in ROW_PLAN.iter().enumerate().skip(1) {
            for col in 0..cols {
                let (_, y) = grid_slot_origin(row, col, GRID);
                assert!(y > source_y, "row {row} must sit below source row 0");
            }
        }

        let (_, client_y) = grid_slot_origin(last_row, 0, GRID);
        for (row, &cols) in ROW_PLAN.iter().enumerate().take(last_row) {
            for col in 0..cols {
                let (_, y) = grid_slot_origin(row, col, GRID);
                assert!(client_y > y, "client row must sit below row {row}");
            }
        }

        let (client_max_x, _) = grid_slot_origin(last_row, last_row_cols - 1, GRID);
        for col in 0..last_row_cols {
            let (x, _) = grid_slot_origin(last_row, col, GRID);
            assert!(
                client_max_x >= x,
                "client row rightmost slot must be max x among clients"
            );
        }
    }

    #[test]
    fn horizontal_and_vertical_gutters_are_even() {
        let (x0, _) = grid_slot_origin(2, 0, GRID);
        let (x1, _) = grid_slot_origin(2, 1, GRID);
        let (x2, _) = grid_slot_origin(2, 2, GRID);
        let gap_a = x1 - x0 - GRID.box_width;
        let gap_b = x2 - x1 - GRID.box_width;
        assert!((gap_a - gap_b).abs() < f64::EPSILON);

        let (_, y0) = grid_slot_origin(0, 0, GRID);
        let (_, y1) = grid_slot_origin(1, 0, GRID);
        let (_, y2) = grid_slot_origin(2, 0, GRID);
        let row_gap_a = y1 - y0 - GRID.box_height;
        let row_gap_b = y2 - y1 - GRID.box_height;
        assert!((row_gap_a - row_gap_b).abs() < f64::EPSILON);
    }

    #[test]
    fn bezier_edge_path_starts_and_ends_at_anchors() {
        let from = slot_anchor(0, 0, Side::Bottom, GRID);
        let to = slot_anchor(1, 0, Side::Top, GRID);
        let d = bezier_edge_path(from, to);
        assert!(d.starts_with(&format!("M{},{}", from.0, from.1)));
        assert!(d.ends_with(&format!("{},{}", to.0, to.1)));
    }

    #[test]
    fn bezier_edge_path_horizontal_edge_starts_and_ends_at_anchors() {
        let from = slot_anchor(1, 0, Side::Right, GRID);
        let to = slot_anchor(1, 2, Side::Left, GRID);
        let d = bezier_edge_path(from, to);
        assert!(d.starts_with(&format!("M{},{}", from.0, from.1)));
        assert!(d.ends_with(&format!("{},{}", to.0, to.1)));
    }

    #[test]
    fn grid_dimensions_bound_every_slot_rect() {
        let max_cols = *ROW_PLAN.iter().max().expect("row plan is non-empty");
        let (width, height) = grid_dimensions(ROW_PLAN.len(), max_cols, GRID);
        for &(row, col) in &all_slots() {
            let (x, y) = grid_slot_origin(row, col, GRID);
            assert!(
                x + GRID.box_width <= width,
                "slot ({row},{col}) exceeds width"
            );
            assert!(
                y + GRID.box_height <= height,
                "slot ({row},{col}) exceeds height"
            );
        }
    }

    #[test]
    fn standard_kind_shows_dots_outflow_utilization_residence_and_polarity() {
        assert!(StockKind::Standard.shows_dots());
        assert!(StockKind::Standard.shows_outflow());
        assert!(StockKind::Standard.shows_utilization());
        assert!(StockKind::Standard.shows_residence());
        assert!(StockKind::Standard.shows_polarity());
    }

    #[test]
    fn bounded_kind_shows_outflow_and_utilization_only() {
        assert!(!StockKind::Bounded.shows_dots());
        assert!(StockKind::Bounded.shows_outflow());
        assert!(StockKind::Bounded.shows_utilization());
        assert!(!StockKind::Bounded.shows_residence());
        assert!(!StockKind::Bounded.shows_polarity());
    }

    #[test]
    fn accumulator_kind_shows_outflow_only() {
        assert!(!StockKind::Accumulator.shows_dots());
        assert!(StockKind::Accumulator.shows_outflow());
        assert!(!StockKind::Accumulator.shows_utilization());
        assert!(!StockKind::Accumulator.shows_residence());
        assert!(!StockKind::Accumulator.shows_polarity());
    }

    #[test]
    fn monotonic_kind_suppresses_everything_but_level_and_inflow() {
        assert!(!StockKind::Monotonic.shows_dots());
        assert!(!StockKind::Monotonic.shows_outflow());
        assert!(!StockKind::Monotonic.shows_utilization());
        assert!(!StockKind::Monotonic.shows_residence());
        assert!(!StockKind::Monotonic.shows_polarity());
    }

    #[test]
    fn level_delta_flows_rising_level_is_pure_inflow() {
        assert_eq!(level_delta_flows(3.0, 7.0), (4.0, 0.0));
    }

    #[test]
    fn level_delta_flows_falling_level_is_pure_outflow() {
        assert_eq!(level_delta_flows(7.0, 3.0), (0.0, 4.0));
    }

    #[test]
    fn level_delta_flows_unchanged_level_is_zero_both() {
        assert_eq!(level_delta_flows(5.0, 5.0), (0.0, 0.0));
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
        assert_eq!(format_rate(3.14042), "3.1");
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
