//! `wasm32`-only reusable rendering-component templates: a small FLAT
//! family of typed handle-structs, each holding its own created DOM
//! handles and its own `mount(container) -> Option<Self>` /
//! `update(...)` pair, mirroring `gh-report-web-client/src/dom.rs` and
//! this crate's [`crate::view`] idiom (raw `web-sys` + leptos reactive
//! primitives, no `view!` macro). Per CHE-0094:R6 there is NO
//! unifying trait over the family — each kind below is a concrete
//! struct with its own `mount`/`update`, sharing only the host-pure
//! layout/format math in [`crate::layout`] and [`crate::sparkline`]
//! (deep shared math core, thin per-kind leaves; a shallow shared
//! trait here would be a COM-0002 over-generalization).
//!
//! Kinds (adr-fmt-vrycy TEMPLATE KINDS NEEDED):
//!
//! 1. [`StockTemplate`] — any [`crate::sd::Stock`] (`WorkQueue`,
//!    `in_flight`, `BatchTracker`, `EvidenceProjection`), including a
//!    [`crate::layout::StockKind::Monotonic`] readout variant
//!    (`generation`, `served_pages`, `events_written`).
//! 2. [`FlowIndicatorTemplate`] — a single flow's rate readout
//!    (arrivals, dequeue, completion, finalize, serve, append,
//!    github-consume).
//! 3. [`ConverterReadoutTemplate`] — a stateless converter's value
//!    (utilization, compression ratio, `barrier_drained`,
//!    `should_reuse`/memo gate).
//! 4. [`CloudBoundaryMarkerTemplate`] — a model-boundary cloud
//!    (`github.com`, web clients, scheduler timer, durable substrate).
//! 5. [`LoopPolarityBadgeTemplate`] — a causal loop's B/R polarity as
//!    badge DATA only (adr-fmt-vrycy found 0 R loops in-boundary; the
//!    R arm is capacity-for-completeness, never a built pipeline).
//!
//! Kind 6 (a non-SD `SweepPhase` phase/control-state overlay) is
//! explicitly OUT OF SCOPE for this module — adr-fmt-vrycy hotspot (c)
//! rules it outside the SD grammar entirely (not a [`crate::sd::Model`]
//! node); it needs a separate annotation mechanism, deferred as a
//! follow-up sub-mission.

use std::cell::Cell;

use web_sys::{Document, Element};

use crate::layout::{self, StockKind};
use crate::sd::{Flow, LevelHistory, LoopPolarity, Terminal};
use crate::sim::JobSource;
use crate::sparkline::polyline_points;
use crate::view::source_color;

const SPARKLINE_WIDTH: f64 = 200.0;
const SPARKLINE_HEIGHT: f64 = 50.0;
const DOTS_HEIGHT: f64 = 18.0;
const DOT_RADIUS: f64 = 4.0;
const SVG_NS: &str = "http://www.w3.org/2000/svg";

/// Kind 1: a generic stock box — one box rendering any
/// [`crate::sd::Stock`]'s history sparkline, level readout, inflow
/// (and for [`StockKind::Standard`] only: live "now" dots, outflow,
/// utilization, mean residence, and loop-polarity) together, driven
/// each tick by the caller through [`Self::update_level`] /
/// [`Self::update_flows`] / [`Self::update_dots`] /
/// [`Self::update_utilization`] / [`Self::update_residence`] /
/// [`Self::update_polarity`]. A [`StockKind::Monotonic`] mount skips
/// the fields a readout-only accumulator (`generation`,
/// `served_pages`, `events_written`) has no value for.
///
/// Holds handles obtained once at [`Self::mount`] time rather than
/// re-querying the DOM by id on every tick.
pub struct StockTemplate {
    polyline: Element,
    dots_layer: Option<Element>,
    level_text: Element,
    inflow_text: Element,
    outflow_text: Option<Element>,
    utilization_text: Option<Element>,
    residence_text: Option<Element>,
    polarity_text: Option<Element>,
    ticks_elapsed: Cell<u64>,
}

impl StockTemplate {
    /// Builds the component's DOM skeleton inside `container` (which
    /// the caller already positioned in the flow graph) for the given
    /// `title` and [`StockKind`], and caches handles to the
    /// sub-elements the `update_*` methods mutate. Returns `None` if
    /// `container` is missing an expected sub-element — treated as
    /// "component did not mount", never a panic.
    #[must_use]
    pub fn mount(container: &Element, title: &str, kind: StockKind) -> Option<Self> {
        container.set_inner_html(&stock_skeleton_markup(title, kind));
        Some(Self {
            polyline: container.query_selector(".sdt-line").ok()??,
            dots_layer: optional_element(container, kind.shows_dots(), ".sdt-dots"),
            level_text: container.query_selector(".sdt-level").ok()??,
            inflow_text: container.query_selector(".sdt-inflow").ok()??,
            outflow_text: optional_element(container, kind.shows_outflow(), ".sdt-outflow"),
            utilization_text: optional_element(container, kind.shows_outflow(), ".sdt-util"),
            residence_text: optional_element(container, kind.shows_residence(), ".sdt-residence"),
            polarity_text: optional_element(container, kind.shows_outflow(), ".sdt-polarity"),
            ticks_elapsed: Cell::new(0),
        })
    }

    /// Re-renders the history sparkline and the level readout text
    /// (caller-formatted: [`layout::format_bounded_level`] for a
    /// capacity-bounded stock, a bare count for a monotonic one).
    pub fn update_level(&self, history: &LevelHistory, level_display: &str) {
        let samples: Vec<f64> = history.iter().collect();
        let points = polyline_points(&samples, SPARKLINE_WIDTH, SPARKLINE_HEIGHT);
        self.polyline.set_attribute("points", &points).ok();
        set_content(&self.level_text, level_display);
    }

    /// Re-renders the inflow rate, and — when this mount's kind
    /// showed an outflow field — the outflow rate too.
    pub fn update_flows(&self, inflow: Flow, outflow: Option<Flow>) {
        set_content(&self.inflow_text, &layout::format_rate(inflow.rate()));
        if let (Some(text), Some(flow)) = (&self.outflow_text, outflow) {
            set_content(text, &layout::format_rate(flow.rate()));
        }
    }

    /// Re-renders the live "now" dots layer from `colors` (one dot
    /// per in-flight unit, already colored by the caller — e.g.
    /// [`crate::view::source_color`] for `WorkQueue` jobs). No-op when
    /// this mount's kind suppressed the dots layer.
    pub fn update_dots(&self, colors: &[&str]) {
        let Some(layer) = &self.dots_layer else {
            return;
        };
        clear_children(layer);
        let Some(document) = layer.owner_document() else {
            return;
        };
        let count = colors.len();
        for (index, color) in colors.iter().enumerate() {
            if let Some(dot) = create_dot(&document, index, count, color) {
                layer.append_child(&dot).ok();
            }
        }
    }

    /// Re-renders the utilization readout from a `0.0..=1.0` fraction.
    /// No-op when this mount's kind suppressed the utilization field.
    pub fn update_utilization(&self, fraction: f64) {
        if let Some(text) = &self.utilization_text {
            set_content(text, &layout::format_percent(fraction));
        }
    }

    /// Re-renders the mean-residence-time readout. No-op when this
    /// mount's kind suppressed the residence field.
    pub fn update_residence(&self, residence_ticks: Option<f64>) {
        if let Some(text) = &self.residence_text {
            set_content(text, &layout::format_residence(residence_ticks));
        }
    }

    /// Re-renders the loop-polarity badge. No-op when this mount's
    /// kind suppressed the polarity field.
    pub fn update_polarity(&self, polarity: LoopPolarity) {
        if let Some(text) = &self.polarity_text {
            set_content(text, layout::polarity_badge_label(polarity));
        }
    }

    /// Advances and returns this mount's elapsed-ticks counter, for
    /// callers computing a Little's-Law mean residence time (e.g.
    /// [`crate::binding::QueueStockBinding::mean_residence_ticks`])
    /// ahead of calling [`Self::update_residence`].
    pub fn tick(&self) -> u64 {
        let next = self.ticks_elapsed.get() + 1;
        self.ticks_elapsed.set(next);
        next
    }
}

/// Kind 2: a single flow's rate readout (arrivals, dequeue,
/// completion, finalize, serve, append, github-consume).
pub struct FlowIndicatorTemplate {
    rate_text: Element,
}

impl FlowIndicatorTemplate {
    #[must_use]
    pub fn mount(container: &Element, label: &str) -> Option<Self> {
        container.set_inner_html(&flow_skeleton_markup(label));
        Some(Self {
            rate_text: container.query_selector(".sdt-rate").ok()??,
        })
    }

    pub fn update(&self, flow: Flow) {
        set_content(&self.rate_text, &layout::format_rate(flow.rate()));
    }
}

/// Kind 3: a stateless converter's readout (utilization, compression
/// ratio, `barrier_drained`, `should_reuse`/memo gate). Takes an
/// already-formatted display string so this one template covers every
/// converter shape in the inventory (percent, ratio, boolean gate)
/// without the template itself knowing any converter's specific unit.
pub struct ConverterReadoutTemplate {
    value_text: Element,
}

impl ConverterReadoutTemplate {
    #[must_use]
    pub fn mount(container: &Element, label: &str) -> Option<Self> {
        container.set_inner_html(&converter_skeleton_markup(label));
        Some(Self {
            value_text: container.query_selector(".sdt-value").ok()??,
        })
    }

    pub fn update(&self, display: &str) {
        set_content(&self.value_text, display);
    }
}

/// Kind 4: a model-boundary cloud marker (`github.com`, web clients,
/// scheduler timer, durable substrate under adr-fmt-vrycy hotspot
/// (a)). The boundary direction glyph is fixed at mount time from the
/// [`Terminal`] kind; only the cumulative tally updates per tick.
pub struct CloudBoundaryMarkerTemplate {
    tally_text: Element,
}

impl CloudBoundaryMarkerTemplate {
    #[must_use]
    pub fn mount(container: &Element, label: &str, terminal: Terminal) -> Option<Self> {
        container.set_inner_html(&cloud_skeleton_markup(label, terminal));
        Some(Self {
            tally_text: container.query_selector(".sdt-tally").ok()??,
        })
    }

    pub fn update(&self, cumulative_count: u64) {
        set_content(&self.tally_text, &cumulative_count.to_string());
    }
}

/// Kind 5: a causal loop's polarity as badge DATA only — B1
/// (backpressure) and B2 (budget-exhaustion) both render through this
/// one template. The `R` label is capacity-for-completeness
/// (adr-fmt-vrycy: 0 R loops found in-boundary); no reinforcing-loop
/// pipeline is built anywhere in this crate.
pub struct LoopPolarityBadgeTemplate {
    badge_text: Element,
}

impl LoopPolarityBadgeTemplate {
    #[must_use]
    pub fn mount(container: &Element, label: &str) -> Option<Self> {
        container.set_inner_html(&loop_badge_skeleton_markup(label));
        Some(Self {
            badge_text: container.query_selector(".sdt-badge").ok()??,
        })
    }

    pub fn update(&self, polarity: LoopPolarity) {
        set_content(&self.badge_text, layout::polarity_badge_label(polarity));
    }
}

fn optional_element(container: &Element, present: bool, selector: &str) -> Option<Element> {
    if !present {
        return None;
    }
    container.query_selector(selector).ok().flatten()
}

fn clear_children(element: &Element) {
    while let Some(child) = element.first_element_child() {
        element.remove_child(&child).ok();
    }
}

fn set_content(element: &Element, value: &str) {
    element.set_text_content(Some(value));
}

/// A single "now" dot for one in-flight unit, positioned along
/// [`SPARKLINE_WIDTH`] by its slice index via
/// [`crate::layout::dot_x`] and colored by the caller-supplied
/// `color` (e.g. [`source_color`] for a [`JobSource`]-colored
/// `WorkQueue` job).
fn create_dot(document: &Document, index: usize, count: usize, color: &str) -> Option<Element> {
    let circle = document.create_element_ns(Some(SVG_NS), "circle").ok()?;
    circle
        .set_attribute("cx", &layout::dot_x(index, count, SPARKLINE_WIDTH).to_string())
        .ok()?;
    circle
        .set_attribute("cy", &(DOTS_HEIGHT / 2.0).to_string())
        .ok()?;
    circle.set_attribute("r", &DOT_RADIUS.to_string()).ok()?;
    circle.set_attribute("fill", color).ok()?;
    Some(circle)
}

/// A dot color for a `WorkQueue` job by [`JobSource`], mirroring
/// [`source_color`] — the thin adapter [`StockTemplate::update_dots`]
/// callers use for the one stock (`WorkQueue`) that has dots at all.
#[must_use]
pub fn job_dot_color(source: JobSource) -> &'static str {
    source_color(source)
}

/// The stock template's DOM skeleton: a title, sparkline SVG, an
/// optional dots SVG, and an inline metrics row whose fields vary by
/// `kind` (class names are unique `sdt-` prefixed so
/// [`StockTemplate::mount`]'s `query_selector` calls resolve
/// unambiguously; suppressed fields are simply absent from the
/// markup, not hidden by CSS).
fn stock_skeleton_markup(title: &str, kind: StockKind) -> String {
    let dots_svg = if kind.shows_dots() {
        r#"<svg class="sdt-dots" viewBox="0 0 200 18" preserveAspectRatio="none"></svg>"#
    } else {
        ""
    };
    let outflow_field = if kind.shows_outflow() {
        r#"<span>out <span class="sdt-outflow">0.0</span></span>
  <span>util <span class="sdt-util">0%</span></span>
  <span>loop <span class="sdt-polarity">B</span></span>"#
    } else {
        ""
    };
    let residence_field = if kind.shows_residence() {
        r#"<span>W <span class="sdt-residence">n/a</span></span>"#
    } else {
        ""
    };
    format!(
        r##"
<div class="sdt-title">{title}</div>
<svg class="sdt-sparkline" viewBox="0 0 200 50" preserveAspectRatio="none">
  <polyline class="sdt-line" points="" fill="none" stroke="#60a5fa" stroke-width="2" />
</svg>
{dots_svg}
<div class="sdt-metrics">
  <span>level <span class="sdt-level">0</span></span>
  <span>in <span class="sdt-inflow">0.0</span></span>
  {outflow_field}
  {residence_field}
</div>
"##
    )
}

fn flow_skeleton_markup(label: &str) -> String {
    format!(
        r#"
<div class="sdt-title">{label}</div>
<div class="sdt-metrics">
  <span>rate <span class="sdt-rate">0.0</span></span>
</div>
"#
    )
}

fn converter_skeleton_markup(label: &str) -> String {
    format!(
        r#"
<div class="sdt-title">{label}</div>
<div class="sdt-metrics">
  <span class="sdt-value">n/a</span>
</div>
"#
    )
}

fn cloud_skeleton_markup(label: &str, terminal: Terminal) -> String {
    let glyph = layout::cloud_direction_glyph(terminal);
    format!(
        r#"
<div class="sdt-title">{label} {glyph}</div>
<div class="sdt-metrics">
  <span>tally <span class="sdt-tally">0</span></span>
</div>
"#
    )
}

fn loop_badge_skeleton_markup(label: &str) -> String {
    format!(
        r#"
<div class="sdt-title">{label}</div>
<div class="sdt-metrics">
  <span class="sdt-badge">B</span>
</div>
"#
    )
}
