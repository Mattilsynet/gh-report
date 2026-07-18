//! `wasm32`-only THIN INTERPRETER (`svs-03`, adr-fmt-sra3p): walks
//! [`crate::scene::gh_report_scene`]'s host-pure [`Scene`] and emits
//! DOM+SVG. Holds ZERO layout/position math of its own — every
//! coordinate rendered here is read from [`Scene::node_origin`],
//! [`Scene::grid`], [`Scene::viewbox_dimensions`], or a
//! [`crate::scene::Belt`]'s already-computed `path` string. A geometry
//! need this module can't satisfy by reading the [`Scene`] belongs in
//! `scene.rs`/`layout.rs` (host-pure, tested), never computed here.
//!
//! Renders the whole scene inside ONE scaling SVG coordinate space
//! (`viewBox` + `preserveAspectRatio`) so the diagram scales as a
//! single unit — responsive, centered, non-overlapping by
//! construction (inherited from `scene.rs`'s own tested no-overlap
//! invariant: uniform scaling preserves relative non-overlap).
//!
//! Live metrics/controls/overlays and belt-item motion are explicitly
//! OUT OF SCOPE for this sub-mission (deferred to `svs-04`/`svs-05`):
//! [`tick`] only advances [`crate::sim::Sim`] so the sim clock stays
//! live for those follow-ups; it renders nothing.

use std::cell::RefCell;

use wasm_bindgen::prelude::wasm_bindgen;

use crate::layout::StockKind;
use crate::scene::{self, Belt, CloudRole, NodeGeometry, PlacedNode, Scene};
use crate::sim::{JobSource, Sim, SimConfig};

/// A [`JobSource`]'s display color, shared by [`crate::components`]'s
/// dot-rendering (`job_dot_color`) even though this module no longer
/// mounts the dots layer itself (deferred to `svs-05`).
pub(crate) fn source_color(source: JobSource) -> &'static str {
    match source {
        JobSource::ScheduledBatch => "#3b82f6",
        JobSource::External { .. } => "#22c55e",
        JobSource::InitialLoad => "#94a3b8",
    }
}

struct AppState {
    sim: Sim,
    tick_count: u64,
}

thread_local! {
    static APP: RefCell<Option<AppState>> = const { RefCell::new(None) };
}

#[wasm_bindgen(start)]
pub fn start() {
    let Some(document) = web_sys::window().and_then(|window| window.document()) else {
        return;
    };
    let Some(root) = document.get_element_by_id("app") else {
        return;
    };

    if let Ok(built_scene) = scene::gh_report_scene() {
        root.set_inner_html(&scene_markup(&built_scene));
    }

    APP.with(|cell| {
        *cell.borrow_mut() = Some(AppState {
            sim: Sim::new(SimConfig::default(), 7),
            tick_count: 0,
        });
    });
}

/// Advances the simulation clock by one tick.
///
/// Invoked from `bootstrap.js` on a `setInterval` cadence, preserved
/// unchanged from the pre-`svs-03` renderer. Renders nothing itself —
/// re-attaching per-tick DOM updates (live sparklines, gauges, belt
/// item motion) is `svs-04`/`svs-05` scope.
#[wasm_bindgen]
pub fn tick() {
    APP.with(|cell| {
        let mut borrowed = cell.borrow_mut();
        let Some(state) = borrowed.as_mut() else {
            return;
        };
        let _events = state.sim.step(false, false);
        state.tick_count += 1;
    });
}

/// The `scene-node-*` CSS class for `geometry`, purely a presentation
/// discriminant (which color/shape family a box belongs to) — never a
/// position.
fn node_class(geometry: NodeGeometry) -> &'static str {
    match geometry {
        NodeGeometry::Stock(_, kind) => match kind {
            StockKind::Standard => "scene-node scene-node-stock scene-node-standard",
            StockKind::Bounded => "scene-node scene-node-stock scene-node-bounded",
            StockKind::Accumulator => "scene-node scene-node-stock scene-node-accumulator",
            StockKind::Monotonic => "scene-node scene-node-stock scene-node-monotonic",
        },
        NodeGeometry::Cloud(_, CloudRole::Source) => {
            "scene-node scene-node-cloud scene-node-source"
        }
        NodeGeometry::Cloud(_, CloudRole::Sink) => "scene-node scene-node-cloud scene-node-sink",
        NodeGeometry::Converter(_) => "scene-node scene-node-converter",
    }
}

/// One placed node's markup: a `foreignObject` sized to the scene's
/// shared box dimensions, positioned at [`Scene::node_origin`],
/// holding a plain labeled `div` — the node box scales together with
/// the rest of the SVG coordinate space rather than sitting in a
/// separate absolute-px HTML layer.
fn node_markup(built_scene: &Scene, node: &PlacedNode) -> String {
    let (x, y) = built_scene.node_origin(node);
    let grid = built_scene.grid();
    let class = node_class(node.geometry);
    format!(
        r#"<foreignObject x="{x}" y="{y}" width="{width}" height="{height}">
  <div xmlns="http://www.w3.org/1999/xhtml" class="{class}">{label}</div>
</foreignObject>
"#,
        width = grid.box_width,
        height = grid.box_height,
        label = node.label,
    )
}

/// One routed belt's markup: an SVG path using [`Belt::path`]'s
/// already-computed `d` string verbatim — no coordinate arithmetic
/// here.
fn belt_markup(belt: &Belt) -> String {
    format!(
        r#"<path class="scene-belt" d="{d}" marker-end="url(#scene-arrow)" />
"#,
        d = belt.path
    )
}

/// The full scene markup: one scaling `<svg>` (`viewBox` +
/// `preserveAspectRatio`) holding a belts layer under a nodes layer,
/// both walked directly off `built_scene` — the renderer's only
/// geometry inputs are [`Scene::viewbox_dimensions`],
/// [`Scene::node_origin`], [`Scene::grid`], and each
/// [`Belt::path`].
fn scene_markup(built_scene: &Scene) -> String {
    let (width, height) = built_scene.viewbox_dimensions();
    let belts: String = built_scene.belts().iter().map(belt_markup).collect();
    let nodes: String = built_scene
        .nodes()
        .iter()
        .map(|node| node_markup(built_scene, node))
        .collect();
    format!(
        r##"<section class="queue-viz">
  <h1>gh-report queue network</h1>
  <div class="scene-viz">
    <svg class="scene-svg" viewBox="0 0 {width} {height}" preserveAspectRatio="xMidYMid meet">
      <defs>
        <marker id="scene-arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
          <path d="M0,0 L10,5 L0,10 z" fill="#94a3b8" />
        </marker>
      </defs>
      <g class="scene-belts">
{belts}      </g>
      <g class="scene-nodes">
{nodes}      </g>
    </svg>
  </div>
</section>
"##
    )
}
