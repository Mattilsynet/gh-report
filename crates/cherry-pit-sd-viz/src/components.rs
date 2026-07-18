//! `wasm32`-only reusable rendering-component abstraction: a typed
//! handle-struct holding its own created DOM handles, mirroring
//! `gh-report-web-client/src/dom.rs` and this crate's [`crate::view`]
//! idiom (raw `web-sys` + leptos reactive primitives, no `view!`
//! macro). [`QueueStockComponent`] is the first instance of the
//! pattern: ONE component box rendering a [`crate::sim::WorkQueue`]'s
//! history sparkline, live "now" dots, and inline key metrics
//! together, proving the pattern is reusable for a later stock/queue
//! pairing beyond `WorkQueue`.
//!
//! The sparkline's point math lives in the host-pure
//! [`crate::sparkline`] module (C1/C7) — this module only creates and
//! mutates DOM nodes from those already-computed points.

use std::cell::Cell;

use web_sys::{Document, Element};

use crate::binding::QueueStockBinding;
use crate::sd::LoopPolarity;
use crate::sim::{JobSource, Sim};
use crate::sparkline::polyline_points;
use crate::view::source_color;

const SPARKLINE_WIDTH: f64 = 200.0;
const SPARKLINE_HEIGHT: f64 = 50.0;
const DOTS_HEIGHT: f64 = 18.0;
const DOT_RADIUS: f64 = 4.0;
const SVG_NS: &str = "http://www.w3.org/2000/svg";

/// A standardized queue/stock component: one box rendering a history
/// sparkline, live "now" dots (one per in-queue job), and inline key
/// metrics together, driven each tick by [`Self::update`].
///
/// Holds handles obtained once at [`Self::mount`] time rather than
/// re-querying the DOM by id on every tick.
pub struct QueueStockComponent {
    polyline: Element,
    dots_layer: Element,
    depth_text: Element,
    inflow_text: Element,
    outflow_text: Element,
    utilization_text: Element,
    residence_text: Element,
    polarity_text: Element,
    ticks_elapsed: Cell<u64>,
}

impl QueueStockComponent {
    /// Builds the component's DOM skeleton inside `container` (which
    /// the caller already positioned in the flow graph) and caches
    /// handles to the sub-elements [`Self::update`] mutates. Returns
    /// `None` if `container` is missing an expected sub-element —
    /// treated as "component did not mount", never a panic.
    #[must_use]
    pub fn mount(container: &Element) -> Option<Self> {
        container.set_inner_html(skeleton_markup());
        Some(Self {
            polyline: container.query_selector(".qsc-line").ok()??,
            dots_layer: container.query_selector(".qsc-dots").ok()??,
            depth_text: container.query_selector(".qsc-depth").ok()??,
            inflow_text: container.query_selector(".qsc-inflow").ok()??,
            outflow_text: container.query_selector(".qsc-outflow").ok()??,
            utilization_text: container.query_selector(".qsc-util").ok()??,
            residence_text: container.query_selector(".qsc-residence").ok()??,
            polarity_text: container.query_selector(".qsc-polarity").ok()??,
            ticks_elapsed: Cell::new(0),
        })
    }

    /// Re-renders all three parts of the component from the current
    /// binding + sim state: the history sparkline, the live "now"
    /// dots, and the inline key metrics.
    pub fn update(&self, binding: &QueueStockBinding, sim: &Sim) {
        self.ticks_elapsed.set(self.ticks_elapsed.get() + 1);
        self.update_sparkline(binding);
        self.update_dots(sim);
        self.update_metrics(binding, sim);
    }

    fn update_sparkline(&self, binding: &QueueStockBinding) {
        let samples: Vec<f64> = binding.level_history().iter().collect();
        let points = polyline_points(&samples, SPARKLINE_WIDTH, SPARKLINE_HEIGHT);
        self.polyline.set_attribute("points", &points).ok();
    }

    fn update_dots(&self, sim: &Sim) {
        clear_children(&self.dots_layer);
        let Some(document) = self.dots_layer.owner_document() else {
            return;
        };
        let jobs = sim.queue_jobs();
        let count = jobs.len();
        for (index, job) in jobs.into_iter().enumerate() {
            if let Some(dot) = create_dot(&document, index, count, job.source) {
                self.dots_layer.append_child(&dot).ok();
            }
        }
    }

    fn update_metrics(&self, binding: &QueueStockBinding, sim: &Sim) {
        set_content(&self.depth_text, &depth_display(sim));
        set_content(
            &self.inflow_text,
            &format!("{:.1}", binding.inflow().rate()),
        );
        set_content(
            &self.outflow_text,
            &format!("{:.1}", binding.outflow().rate()),
        );
        set_content(&self.utilization_text, &utilization_display(binding));
        set_content(
            &self.residence_text,
            &residence_display(binding, self.ticks_elapsed.get()),
        );
        set_content(&self.polarity_text, polarity_label(binding));
    }
}

fn depth_display(sim: &Sim) -> String {
    format!("{}/{}", sim.queue_depth(), sim.queue_capacity())
}

fn utilization_display(binding: &QueueStockBinding) -> String {
    format!("{:.0}%", binding.utilization().value() * 100.0)
}

fn residence_display(binding: &QueueStockBinding, ticks_elapsed: u64) -> String {
    binding
        .mean_residence_ticks(ticks_elapsed)
        .map_or_else(|| "n/a".to_string(), |ticks| format!("{ticks:.1}"))
}

fn polarity_label(binding: &QueueStockBinding) -> &'static str {
    match binding.backpressure_polarity() {
        LoopPolarity::Reinforcing => "R",
        LoopPolarity::Balancing => "B",
    }
}

fn clear_children(element: &Element) {
    while let Some(child) = element.first_element_child() {
        element.remove_child(&child).ok();
    }
}

fn set_content(element: &Element, value: &str) {
    element.set_text_content(Some(value));
}

/// A single "now" dot for one in-queue job, positioned along
/// [`SPARKLINE_WIDTH`] by its FIFO position and colored by
/// [`JobSource`] (mirrors [`crate::view::source_color`]).
#[expect(
    clippy::cast_precision_loss,
    reason = "in-queue job counts are bounded well under 2^52 for any realistic queue capacity"
)]
fn create_dot(
    document: &Document,
    index: usize,
    count: usize,
    source: JobSource,
) -> Option<Element> {
    let circle = document.create_element_ns(Some(SVG_NS), "circle").ok()?;
    let cx = SPARKLINE_WIDTH * (index as f64 + 0.5) / count as f64;
    circle.set_attribute("cx", &cx.to_string()).ok()?;
    circle
        .set_attribute("cy", &(DOTS_HEIGHT / 2.0).to_string())
        .ok()?;
    circle.set_attribute("r", &DOT_RADIUS.to_string()).ok()?;
    circle.set_attribute("fill", source_color(source)).ok()?;
    Some(circle)
}

/// The component's DOM skeleton: a title, sparkline SVG, dots SVG, and
/// an inline metrics row, all inside `container` (already positioned
/// by the caller). Class names are unique (`qsc-` prefix) so
/// [`QueueStockComponent::mount`]'s `query_selector` calls resolve
/// unambiguously.
fn skeleton_markup() -> &'static str {
    r##"
<div class="qsc-title">WorkQueue</div>
<svg class="qsc-sparkline" viewBox="0 0 200 50" preserveAspectRatio="none">
  <polyline class="qsc-line" points="" fill="none" stroke="#60a5fa" stroke-width="2" />
</svg>
<svg class="qsc-dots" viewBox="0 0 200 18" preserveAspectRatio="none"></svg>
<div class="qsc-metrics">
  <span>depth <span class="qsc-depth">0/0</span></span>
  <span>in <span class="qsc-inflow">0.0</span></span>
  <span>out <span class="qsc-outflow">0.0</span></span>
  <span>util <span class="qsc-util">0%</span></span>
  <span>W <span class="qsc-residence">n/a</span></span>
  <span>loop <span class="qsc-polarity">B</span></span>
</div>
"##
}
