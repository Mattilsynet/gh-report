//! `wasm32`-only THIN INTERPRETER (`svs-03`/`svs-04`, adr-fmt-sra3p):
//! walks [`crate::scene::gh_report_scene`]'s host-pure [`Scene`] and
//! emits DOM+SVG. Holds ZERO layout/position/motion math of its own —
//! every coordinate rendered here is read from [`Scene::node_origin`],
//! [`Scene::grid`], [`Scene::viewbox_dimensions`], a
//! [`crate::scene::Belt`]'s already-computed `path`/`length`/`kind`, or
//! one of [`crate::scene::belt_item_count`]/[`crate::scene::belt_item_phase`]/
//! [`crate::scene::fill_fraction`]/[`crate::scene::belt_activity_step`]
//! (all host-pure, tested in `scene.rs`). A geometry or motion need
//! this module can't satisfy by reading the [`Scene`] or calling one
//! of those functions belongs in `scene.rs`/`layout.rs`, never computed
//! here.
//!
//! Renders the whole scene inside ONE scaling SVG coordinate space
//! (`viewBox` + `preserveAspectRatio`) so the diagram scales as a
//! single unit — responsive, centered, non-overlapping by
//! construction (inherited from `scene.rs`'s own tested no-overlap
//! invariant: uniform scaling preserves relative non-overlap).
//!
//! # Belt items (`svs-04`)
//!
//! Every belt renders a shapez2-style continuous stream of
//! evenly-spaced item circles: [`crate::scene::belt_item_count`] picks
//! how many fit at [`ITEM_SPACING`], [`crate::scene::belt_item_phase`]
//! places each one, and [`crate::scene::Belt::point_at`] converts that
//! fraction to a scene-space point — the renderer never computes a
//! position itself, only feeds these host-pure functions a `speed`
//! signal. The three WorkQueue-arrival belts and the WorkQueue-drain
//! belt track a live per-tick "activity" speed
//! ([`crate::scene::belt_activity_step`]) driven by the actual sim
//! events [`tick`] triggers this turn (arrivals/dequeues), so those
//! four belts visibly speed up right when an item enters or leaves the
//! queue rather than running at a decorative constant; every other
//! belt runs at [`IDLE_SPEED`]. The `WorkQueue` box additionally renders
//! a fill gauge from [`crate::scene::fill_fraction`] over
//! [`crate::sim::Sim::queue_depth`]/`queue_capacity`, so inflow
//! visibly outpacing drain (or vice versa) shows up as the gauge
//! rising or falling, not just as moving dots.
//!
//! Live metrics/controls/overlays beyond this (sparkline, `SweepPhase`
//! overlay, warm-start, backend toggle, rate controls, inventory/
//! `should_reuse`, ws-permits/github-budget) are explicitly OUT OF
//! SCOPE for this sub-mission (deferred to `svs-05`).

use std::cell::RefCell;

use wasm_bindgen::prelude::wasm_bindgen;

use crate::layout::StockKind;
use crate::scene::{
    self, Belt, CloudRole, NodeGeometry, PlacedNode, Scene, belt_activity_step, belt_item_count,
    belt_item_phase, fill_fraction, fill_rect_geometry,
};
use crate::sim::{JobSource, Sim, SimConfig, WebhookKind};

/// A [`JobSource`]'s display color, shared by [`crate::components`]'s
/// dot-rendering (`job_dot_color`) and this module's belt-item
/// rendering.
pub(crate) fn source_color(source: JobSource) -> &'static str {
    match source {
        JobSource::ScheduledBatch => "#3b82f6",
        JobSource::External { .. } => "#22c55e",
        JobSource::InitialLoad => "#94a3b8",
    }
}

/// Fractional px a belt's evenly-spaced items sit apart —
/// [`belt_item_count`]'s `spacing` argument.
const ITEM_SPACING: f64 = 34.0;

/// Item circle radius, px.
const ITEM_RADIUS: f64 = 6.0;

/// A belt's motion speed (px per tick) when its "activity" signal has
/// fully decayed — every belt keeps drifting at this idle rate rather
/// than stalling, matching the shapez2 always-running-conveyor feel.
const IDLE_SPEED: f64 = 6.0;

/// A belt's motion speed (px per tick) the tick a matching sim event
/// fires — [`belt_activity_step`]'s `boost` argument.
const ACTIVITY_BOOST: f64 = 48.0;

/// Per-tick falloff [`belt_activity_step`] applies when no matching
/// event fires this tick.
const ACTIVITY_DECAY: f64 = 0.85;

/// Deterministic scheduled-batch arrival cadence: every `N`th tick
/// submits one [`JobSource::ScheduledBatch`] job. `svs-05` replaces
/// this with the live rate-control signal the pre-redesign renderer
/// exposed (`rates.batch_per_tick`); until then a fixed cadence keeps
/// the sim (and therefore the belt/queue visual) actually running
/// rather than permanently idle, per `commander_intent`'s "reflects the
/// running sim, not a decorative constant".
const BATCH_ARRIVAL_EVERY_N_TICKS: u64 = 3;

/// Deterministic external-webhook arrival cadence (see
/// [`BATCH_ARRIVAL_EVERY_N_TICKS`]).
const EXTERNAL_ARRIVAL_EVERY_N_TICKS: u64 = 5;

/// Live per-tick "activity" speed for the belts this module tracks by
/// hand: the three WorkQueue-arrival belts (indexed by
/// [`crate::binding::tier1_model`]'s registration order — `0`
/// scheduled-batch, `1` external, `2` initial-load) plus the
/// WorkQueue-drain belt (`3`). All four start at [`IDLE_SPEED`] and
/// bump to [`ACTIVITY_BOOST`] on a matching event via
/// [`belt_activity_step`].
struct BeltActivity {
    scheduled_batch: f64,
    external: f64,
    initial_load: f64,
    drain: f64,
}

impl BeltActivity {
    fn idle() -> Self {
        Self {
            scheduled_batch: IDLE_SPEED,
            external: IDLE_SPEED,
            initial_load: IDLE_SPEED,
            drain: IDLE_SPEED,
        }
    }

    /// Advances every tracked belt's activity signal for one tick,
    /// given whether a matching sim event fired this tick.
    fn step(&mut self, batch_fired: bool, external_fired: bool, drain_fired: bool) {
        self.scheduled_batch = belt_activity_step(
            self.scheduled_batch,
            batch_fired,
            ACTIVITY_BOOST,
            ACTIVITY_DECAY,
            IDLE_SPEED,
        );
        self.external = belt_activity_step(
            self.external,
            external_fired,
            ACTIVITY_BOOST,
            ACTIVITY_DECAY,
            IDLE_SPEED,
        );
        self.initial_load = belt_activity_step(
            self.initial_load,
            false,
            ACTIVITY_BOOST,
            ACTIVITY_DECAY,
            IDLE_SPEED,
        );
        self.drain = belt_activity_step(
            self.drain,
            drain_fired,
            ACTIVITY_BOOST,
            ACTIVITY_DECAY,
            IDLE_SPEED,
        );
    }

    /// The `(speed, color)` pair a belt at `index` in
    /// [`Scene::belts`]'s registration order animates with. The first
    /// four indices are the hand-tracked `WorkQueue` arrival/drain
    /// belts, each colored by its representative [`JobSource`]; every
    /// other belt runs at [`IDLE_SPEED`] in the neutral belt color.
    fn speed_and_color_for(&self, index: usize) -> (f64, &'static str) {
        match index {
            0 => (
                self.scheduled_batch,
                source_color(JobSource::ScheduledBatch),
            ),
            1 => (
                self.external,
                source_color(JobSource::External {
                    id: 0,
                    kind: WebhookKind::Push,
                }),
            ),
            2 => (self.initial_load, source_color(JobSource::InitialLoad)),
            3 => (self.drain, "#94a3b8"),
            _ => (IDLE_SPEED, "#94a3b8"),
        }
    }
}

struct AppState {
    sim: Sim,
    scene: Scene,
    tick_count: u64,
    activity: BeltActivity,
    last_worker_executions: u64,
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

    let Ok(built_scene) = scene::gh_report_scene() else {
        return;
    };
    let sim = Sim::new(SimConfig::default(), 7);
    root.set_inner_html(&scene_markup(&built_scene, &sim, 0, &BeltActivity::idle()));

    APP.with(|cell| {
        *cell.borrow_mut() = Some(AppState {
            sim,
            scene: built_scene,
            tick_count: 0,
            activity: BeltActivity::idle(),
            last_worker_executions: 0,
        });
    });
}

/// Advances the simulation clock by one tick and re-renders the belt
/// items + `WorkQueue` fill gauge from the freshly-advanced state.
///
/// Invoked from `bootstrap.js` on a `setInterval` cadence, preserved
/// unchanged from the pre-`svs-04` renderer. Re-attaching the rest of
/// the live metrics/controls/overlays is `svs-05` scope.
#[wasm_bindgen]
pub fn tick() {
    let Some(document) = web_sys::window().and_then(|window| window.document()) else {
        return;
    };
    let Some(root) = document.get_element_by_id("app") else {
        return;
    };

    APP.with(|cell| {
        let mut borrowed = cell.borrow_mut();
        let Some(state) = borrowed.as_mut() else {
            return;
        };

        let batch_fired = state.tick_count.is_multiple_of(BATCH_ARRIVAL_EVERY_N_TICKS);
        let external_fired = state
            .tick_count
            .is_multiple_of(EXTERNAL_ARRIVAL_EVERY_N_TICKS);
        let _events = state.sim.step(batch_fired, external_fired);
        state.tick_count += 1;

        let worker_executions = state.sim.worker_executions();
        let drain_fired = worker_executions > state.last_worker_executions;
        state.last_worker_executions = worker_executions;

        state
            .activity
            .step(batch_fired, external_fired, drain_fired);

        root.set_inner_html(&scene_markup(
            &state.scene,
            &state.sim,
            state.tick_count,
            &state.activity,
        ));
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

/// One belt's evenly-spaced item circles at `tick_count`, moving at
/// `speed` and colored `color` — every position comes from
/// [`belt_item_count`]/[`belt_item_phase`]/[`Belt::point_at`] (all
/// host-pure); this function only turns the resulting `(x, y)` pairs
/// into `<circle>` markup.
fn belt_items_markup(belt: &Belt, tick_count: u64, speed: f64, color: &str) -> String {
    use std::fmt::Write as _;

    let count = belt_item_count(belt.length, ITEM_SPACING);
    #[expect(
        clippy::cast_precision_loss,
        reason = "tick counts are bounded well under 2^52 for any realistic sim run"
    )]
    let t = tick_count as f64;
    (0..count).fold(String::new(), |mut markup, k| {
        let phase = belt_item_phase(k, t, speed, belt.length, ITEM_SPACING);
        let (x, y) = belt.point_at(phase);
        let _ = writeln!(
            markup,
            r#"<circle class="scene-item" cx="{x}" cy="{y}" r="{ITEM_RADIUS}" fill="{color}" />"#
        );
        markup
    })
}

/// The `WorkQueue` node (the unique [`StockKind::Standard`] placed
/// stock) plus its fill-gauge rect, or an empty string when the scene
/// carries no such node (defensive — every `gh_report_scene` build
/// has exactly one).
fn work_queue_fill_markup(built_scene: &Scene, sim: &Sim) -> String {
    let Some(node) = built_scene
        .nodes()
        .iter()
        .find(|node| matches!(node.geometry, NodeGeometry::Stock(_, StockKind::Standard)))
    else {
        return String::new();
    };
    let (x, y) = built_scene.node_origin(node);
    let grid = built_scene.grid();
    #[expect(
        clippy::cast_precision_loss,
        reason = "queue depth/capacity are bounded well under 2^52 for any realistic sim run"
    )]
    let fraction = fill_fraction(sim.queue_depth() as f64, sim.queue_capacity() as f64);
    let (fill_y, fill_height) = fill_rect_geometry((x, y), grid.box_height, fraction);
    format!(
        r#"<rect class="scene-queue-fill" x="{x}" y="{fill_y}" width="{width}" height="{fill_height}" />
"#,
        width = grid.box_width,
    )
}

/// The full scene markup: one scaling `<svg>` (`viewBox` +
/// `preserveAspectRatio`) holding a belts layer, a `WorkQueue` fill
/// gauge, a belt-items layer, and a nodes layer — all walked directly
/// off `built_scene`/`sim`/`activity`. The renderer's only geometry
/// and motion inputs are [`Scene::viewbox_dimensions`],
/// [`Scene::node_origin`], [`Scene::grid`], each [`Belt::path`], and
/// the host-pure [`belt_item_count`]/[`belt_item_phase`]/
/// [`fill_fraction`] functions.
fn scene_markup(
    built_scene: &Scene,
    sim: &Sim,
    tick_count: u64,
    activity: &BeltActivity,
) -> String {
    let (width, height) = built_scene.viewbox_dimensions();
    let belts: String = built_scene.belts().iter().map(belt_markup).collect();
    let items: String = built_scene
        .belts()
        .iter()
        .enumerate()
        .map(|(index, belt)| {
            let (speed, color) = activity.speed_and_color_for(index);
            belt_items_markup(belt, tick_count, speed, color)
        })
        .collect();
    let queue_fill = work_queue_fill_markup(built_scene, sim);
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
      <g class="scene-queue-fill-layer">
{queue_fill}      </g>
      <g class="scene-items">
{items}      </g>
      <g class="scene-nodes">
{nodes}      </g>
    </svg>
  </div>
</section>
"##
    )
}
