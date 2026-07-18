//! `wasm32`-only THIN INTERPRETER (`svs-03`..`svs-05`, adr-fmt-sra3p):
//! walks [`crate::scene::gh_report_scene`]'s host-pure [`Scene`] and
//! emits DOM+SVG. Holds ZERO layout/position/motion math of its own —
//! every coordinate rendered here is read from [`Scene::node_origin`],
//! [`Scene::grid`], [`Scene::viewbox_dimensions`], a
//! [`crate::scene::Belt`]'s already-computed `path`/`length`/`kind`, or
//! one of [`crate::scene::belt_item_count`]/[`crate::scene::belt_item_phase`]/
//! [`crate::scene::fill_fraction`]/[`crate::scene::belt_activity_step`]/
//! [`crate::sparkline::polyline_points`] (all host-pure, tested outside
//! this module). A geometry, motion, or history need this module can't
//! satisfy by reading the [`Scene`]/[`Sim`] or calling one of those
//! functions belongs in `scene.rs`/`layout.rs`/`sparkline.rs`, never
//! computed here.
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
//! fraction to a scene-space point. The three `WorkQueue`-arrival belts
//! and the `WorkQueue`-drain belt track a live per-tick "activity"
//! speed ([`crate::scene::belt_activity_step`]) driven by the actual
//! sim events [`tick`] triggers this turn (arrivals/dequeues), so those
//! four belts visibly speed up right when an item enters or leaves the
//! queue; every other belt runs at [`IDLE_SPEED`].
//!
//! # Live flow-rate wiring (`svs-05`, folded in from the `svs-04`
//! back-brief)
//!
//! [`crate::binding::tier1_model`]'s 11 flows start as placeholder
//! `Flow::Uniflow(0.0)` edges. [`tick`] now overwrites the Tier-1
//! SPINE flows' rates every tick via [`Scene::set_flow_rate`], derived
//! from the same events [`Sim::step`] returns this tick (arrivals by
//! source, [`crate::binding::QueueStockBinding`]'s measured dequeue
//! outflow, completions, and finalize/`page_updates`), so
//! `belt.kind.rate()` is live and correct for any reader (the
//! `Utilization`/`ReadSide` converters, a future debug readout) even
//! though the belt ANIMATION speed still comes from
//! [`BeltActivity`]'s smoothed signal (kept — an instantaneous
//! per-tick 0/1 rate fed straight into [`crate::scene::belt_item_phase`]
//! would visibly teleport items rather than ease, regressing the
//! shapez2 feel `commander_intent` requires). The three boundary flows
//! this crate has no per-tick measured signal for yet (`github`-consume,
//! `served_pages` to clients, `evidence_projection` to durable storage
//! and `events_written`) are left on their `Flow::Uniflow(0.0)`
//! placeholder — out of Tier-1-spine scope per `svs-05`'s contract.
//!
//! # Controls, readouts, sparklines, overlay (`svs-05`)
//!
//! `#app` mounts two children ONCE in [`start`]: `#scene-mount` (the
//! belts/items/nodes SVG, fully regenerated every [`tick`] — matches
//! `svs-03`/`svs-04`) and `#controls-hud` (warm-start/backend-toggle/
//! rate buttons, wired ONCE with their `web_sys` click listeners so a
//! full-innerHTML scene regen never detaches them; every readout
//! inside it is updated per-tick via targeted `set_text_content`
//! calls, never a full-panel re-mount). Each `Stock` node's box gets a
//! small inline sparkline (`crate::sparkline::polyline_points` over a
//! per-stock [`LevelHistory`] this module records once per tick) plus
//! its numeric level — both read-only, so they ride the same full
//! scene-string regen with no listener-detachment risk. The
//! `SweepPhase` control-state (adr-fmt-vrycy hotspot (c): NOT an
//! `sd::Model` element) renders as a plain annotation badge positioned
//! at the `BatchRemaining` stock's already-placed origin, sharing no
//! `scene-node-*` class with the wired SD nodes.

use std::cell::RefCell;

use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::{Closure, wasm_bindgen};
use web_sys::Document;

use crate::binding::QueueStockBinding;
use crate::layout::{StockKind, format_bounded_level};
use crate::scene::{
    self, Belt, CloudRole, NodeGeometry, PlacedNode, Scene, belt_activity_step, belt_item_count,
    belt_item_phase, fill_fraction, fill_rect_geometry,
};
use crate::sd::{FlowId, LevelHistory};
use crate::sim::{
    EnqueueResult, JobOutcome, JobSource, PardosaBackend, Sim, SimConfig, UpdatedAt, WebhookKind,
};

/// A [`JobSource`]'s display color, shared by belt-item rendering.
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

/// Every `N`th tick this many ticks apart submits a sweep-triggered
/// `ScheduledBatch`/`External` arrival — the live rate-control signal
/// [`Rates`] exposes via the batch/external rate buttons.
const DEFAULT_BATCH_PER_TICK: u32 = 3;
const DEFAULT_EXTERNAL_PER_TICK: u32 = 5;
const MAX_RATE_PER_TICK: u32 = 10;

/// How many ticks apart the deterministic inventory sweep
/// ([`inventory_repos`]) fires, refreshing the inventory/`should_reuse`
/// readout independent of the arrival cadence above.
const INVENTORY_SWEEP_EVERY_N_TICKS: u64 = 20;

/// How many ticks apart the demo ws-client pool connects/disconnects
/// one client, so the ws-permits readout visibly moves.
const CLIENT_CONNECT_EVERY_N_TICKS: u64 = 7;
const CLIENT_DISCONNECT_EVERY_N_TICKS: u64 = 11;

/// Capacity of each Tier-1 stock's per-tick [`LevelHistory`] sparkline
/// window.
const STOCK_HISTORY_CAPACITY: usize = 120;

/// SVG px size of one inline stock sparkline.
const SPARKLINE_WIDTH: f64 = 140.0;
const SPARKLINE_HEIGHT: f64 = 22.0;

/// Live per-tick "activity" speed for the belts this module tracks by
/// hand: the three `WorkQueue`-arrival belts (indexed by
/// [`crate::binding::tier1_model`]'s registration order — `0`
/// scheduled-batch, `1` external, `2` initial-load) plus the
/// `WorkQueue`-drain belt (`3`). All four start at [`IDLE_SPEED`] and
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

/// Live batch/external arrival cadence, adjustable via the rate
/// buttons. `1` ticks means "every tick"; clamped to
/// `[1, MAX_RATE_PER_TICK]`.
#[derive(Clone, Copy)]
struct Rates {
    batch_per_tick: u32,
    external_per_tick: u32,
}

impl Rates {
    const fn clamp(self) -> Self {
        Self {
            batch_per_tick: clamp_rate(self.batch_per_tick),
            external_per_tick: clamp_rate(self.external_per_tick),
        }
    }
}

const fn clamp_rate(value: u32) -> u32 {
    if value < 1 {
        1
    } else if value > MAX_RATE_PER_TICK {
        MAX_RATE_PER_TICK
    } else {
        value
    }
}

impl Default for Rates {
    fn default() -> Self {
        Self {
            batch_per_tick: DEFAULT_BATCH_PER_TICK,
            external_per_tick: DEFAULT_EXTERNAL_PER_TICK,
        }
    }
}

/// Which [`Rates`] field a rate button adjusts.
#[derive(Clone, Copy)]
enum RateField {
    Batch,
    External,
}

/// The [`FlowId`]s of the Tier-1 spine flows [`tick`] overwrites every
/// tick with a live measured rate, captured once at [`start`] from
/// [`Scene::belts`]'s registration order (matches
/// [`crate::binding::tier1_model`]'s `connect_flow` call order:
/// `0..=2` the three `WorkQueue` arrivals, `3` dequeue, `4` completion,
/// `6` finalize). `5` (github-consume), `7..=10` (the read-side/serve/
/// durable/events-written boundary flows) have no per-tick measured
/// signal in this crate yet and stay on their `Flow::Uniflow(0.0)`
/// placeholder — out of Tier-1-spine scope per `svs-05`'s contract.
struct SpineFlowIds {
    scheduled_batch: FlowId,
    external: FlowId,
    initial_load: FlowId,
    dequeue: FlowId,
    completion: FlowId,
    finalize: FlowId,
}

impl SpineFlowIds {
    fn from_scene(built_scene: &Scene) -> Self {
        let belts = built_scene.belts();
        Self {
            scheduled_batch: belts[0].id,
            external: belts[1].id,
            initial_load: belts[2].id,
            dequeue: belts[3].id,
            completion: belts[4].id,
            finalize: belts[6].id,
        }
    }
}

struct AppState {
    sim: Sim,
    scene: Scene,
    tick_count: u64,
    activity: BeltActivity,
    last_worker_executions: u64,
    queue_binding: QueueStockBinding,
    spine_flows: SpineFlowIds,
    stock_histories: Vec<LevelHistory>,
    rates: Rates,
    warm_start_requested: bool,
    backend_toggle_requested: bool,
    inventory_epoch: u64,
}

thread_local! {
    static APP: RefCell<Option<AppState>> = const { RefCell::new(None) };
    static CLOSURES: RefCell<Vec<Closure<dyn FnMut()>>> = const { RefCell::new(Vec::new()) };
}

/// Each of [`crate::binding::tier1_model`]'s 7 stocks' current level,
/// in the same insertion order [`crate::scene::Scene::nodes`] places
/// them (`work_queue`, `in_flight`, `batch_remaining`,
/// `evidence_projection`, `generation`, `served_pages`,
/// `events_written`).
#[expect(
    clippy::cast_precision_loss,
    reason = "sim counters (queue depth, in-flight, batch remaining, repos captured, generation, served pages, events written) are bounded well under 2^52 for any realistic sim run"
)]
fn tier1_stock_levels(sim: &Sim) -> [f64; 7] {
    [
        sim.queue_depth() as f64,
        sim.in_flight() as f64,
        sim.batch_remaining() as f64,
        sim.repositories_captured() as f64,
        sim.arcswap_generation() as f64,
        sim.served_pages() as f64,
        sim.events_written() as f64,
    ]
}

/// Deterministic synthetic repo inventory for the periodic
/// `should_reuse` sweep readout: half the repos "changed" (spawn a
/// job), half unchanged (reused from baseline), rotating which half
/// via `epoch` so the split visibly varies sweep to sweep.
fn inventory_repos(epoch: u64) -> Vec<(UpdatedAt, UpdatedAt)> {
    (0..10)
        .map(|i| {
            let baseline = UpdatedAt(Some(i));
            let current = if (i + epoch).is_multiple_of(2) {
                UpdatedAt(Some(i))
            } else {
                UpdatedAt(Some(i + 100 + epoch))
            };
            (baseline, current)
        })
        .collect()
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
    let spine_flows = SpineFlowIds::from_scene(&built_scene);
    let stock_count = built_scene
        .nodes()
        .iter()
        .filter(|node| matches!(node.geometry, NodeGeometry::Stock(..)))
        .count();
    let stock_histories: Vec<LevelHistory> = (0..stock_count)
        .map(|_| LevelHistory::new(STOCK_HISTORY_CAPACITY))
        .collect();

    root.set_inner_html(r#"<div id="scene-mount"></div><div id="controls-hud"></div>"#);
    let Some(scene_mount) = document.get_element_by_id("scene-mount") else {
        return;
    };
    let Some(controls_hud) = document.get_element_by_id("controls-hud") else {
        return;
    };
    scene_mount.set_inner_html(&scene_markup(
        &built_scene,
        &sim,
        0,
        &BeltActivity::idle(),
        &stock_histories,
    ));
    controls_hud.set_inner_html(&controls_markup(&sim));

    wire_controls(&document);

    let queue_binding = QueueStockBinding::new(&sim);
    APP.with(|cell| {
        *cell.borrow_mut() = Some(AppState {
            sim,
            scene: built_scene,
            tick_count: 0,
            activity: BeltActivity::idle(),
            last_worker_executions: 0,
            queue_binding,
            spine_flows,
            stock_histories,
            rates: Rates::default(),
            warm_start_requested: false,
            backend_toggle_requested: false,
            inventory_epoch: 0,
        });
    });
}

/// Wires every control's click listener exactly once, against the
/// `#controls-hud` markup [`controls_markup`] just mounted. A closure
/// only ever flips a request flag / adjusts [`Rates`] on the shared
/// [`AppState`] — [`tick`] is the sole place that acts on a request,
/// keeping "what a click means" and "when it takes effect" in one
/// place. Closures are leaked into [`CLOSURES`] (not dropped) so
/// `web_sys` keeps their `wasm-bindgen` shims alive for the page's
/// lifetime — matches the pre-redesign renderer's own convention.
fn wire_controls(document: &Document) {
    wire_click(document, "warm-start-btn", || {
        with_app(|state| state.warm_start_requested = true);
    });
    wire_click(document, "backend-toggle-btn", || {
        with_app(|state| state.backend_toggle_requested = true);
    });
    wire_rate_button(document, "batch-rate-down", RateField::Batch, -1);
    wire_rate_button(document, "batch-rate-up", RateField::Batch, 1);
    wire_rate_button(document, "external-rate-down", RateField::External, -1);
    wire_rate_button(document, "external-rate-up", RateField::External, 1);
}

fn with_app(action: impl FnOnce(&mut AppState)) {
    APP.with(|cell| {
        if let Some(state) = cell.borrow_mut().as_mut() {
            action(state);
        }
    });
}

fn wire_click(document: &Document, element_id: &str, action: impl FnMut() + 'static) {
    let Some(element) = document.get_element_by_id(element_id) else {
        return;
    };
    let closure = Closure::<dyn FnMut()>::new(action);
    if element
        .add_event_listener_with_callback("click", closure.as_ref().unchecked_ref())
        .is_ok()
    {
        CLOSURES.with(|cell| cell.borrow_mut().push(closure));
    }
}

fn wire_rate_button(document: &Document, element_id: &str, field: RateField, delta: i32) {
    wire_click(document, element_id, move || {
        with_app(|state| {
            let current = state.rates;
            let adjusted = match field {
                RateField::Batch => Rates {
                    batch_per_tick: current.batch_per_tick.saturating_add_signed(delta),
                    ..current
                },
                RateField::External => Rates {
                    external_per_tick: current.external_per_tick.saturating_add_signed(delta),
                    ..current
                },
            };
            state.rates = adjusted.clamp();
        });
        if let Some(document) = web_sys::window().and_then(|window| window.document()) {
            with_app(|state| render_rate_readouts(&document, state.rates));
        }
    });
}

fn render_rate_readouts(document: &Document, rates: Rates) {
    set_text(
        document,
        "batch-rate-value",
        &rates.batch_per_tick.to_string(),
    );
    set_text(
        document,
        "external-rate-value",
        &rates.external_per_tick.to_string(),
    );
}

fn set_text(document: &Document, element_id: &str, value: &str) {
    if let Some(element) = document.get_element_by_id(element_id) {
        element.set_text_content(Some(value));
    }
}

/// Advances the simulation clock by one tick, re-renders `#scene-mount`
/// (belts/items/`WorkQueue` fill gauge/nodes/sparklines/`SweepPhase`
/// annotation) and refreshes every targeted readout in `#controls-hud`.
///
/// Invoked from `bootstrap.js` on a `setInterval` cadence, preserved
/// unchanged from the pre-`svs-05` renderer.
#[wasm_bindgen]
pub fn tick() {
    let Some(document) = web_sys::window().and_then(|window| window.document()) else {
        return;
    };
    let Some(scene_mount) = document.get_element_by_id("scene-mount") else {
        return;
    };

    APP.with(|cell| {
        let mut borrowed = cell.borrow_mut();
        let Some(state) = borrowed.as_mut() else {
            return;
        };

        if state.warm_start_requested {
            state.warm_start_requested = false;
            let _ = state.sim.warm_start();
        }
        if state.backend_toggle_requested {
            state.backend_toggle_requested = false;
            let next = match state.sim.durable_backend() {
                PardosaBackend::Pgno => PardosaBackend::Nats,
                PardosaBackend::Nats => PardosaBackend::Pgno,
            };
            state.sim.set_durable_backend(next);
        }

        if state
            .tick_count
            .is_multiple_of(CLIENT_CONNECT_EVERY_N_TICKS)
        {
            let _ = state.sim.connect_client();
        }
        if state
            .tick_count
            .is_multiple_of(CLIENT_DISCONNECT_EVERY_N_TICKS)
            && state.sim.ws_permits_in_use() > 0
        {
            state.sim.disconnect_client();
        }

        if state
            .tick_count
            .is_multiple_of(INVENTORY_SWEEP_EVERY_N_TICKS)
        {
            let repos = inventory_repos(state.inventory_epoch);
            state.inventory_epoch += 1;
            let _ = state.sim.run_inventory_sweep(&repos, false);
        }

        let batch_fired = state
            .tick_count
            .is_multiple_of(u64::from(state.rates.batch_per_tick));
        let external_fired = state
            .tick_count
            .is_multiple_of(u64::from(state.rates.external_per_tick));
        let events = state.sim.step(batch_fired, external_fired);
        state.tick_count += 1;
        state.queue_binding.advance(&events, &state.sim);

        let worker_executions = state.sim.worker_executions();
        let drain_fired = worker_executions > state.last_worker_executions;
        state.last_worker_executions = worker_executions;

        state
            .activity
            .step(batch_fired, external_fired, drain_fired);

        apply_live_flow_rates(state, &events);

        let levels = tier1_stock_levels(&state.sim);
        for (history, level) in state.stock_histories.iter_mut().zip(levels) {
            history.push(level);
        }

        scene_mount.set_inner_html(&scene_markup(
            &state.scene,
            &state.sim,
            state.tick_count,
            &state.activity,
            &state.stock_histories,
        ));
        render_hud_readouts(&document, state);
    });
}

/// Overwrites the Tier-1 spine flows' rates from this tick's actual
/// measured activity (see module doc "Live flow-rate wiring"):
/// arrivals split by source, [`QueueStockBinding`]'s measured dequeue
/// outflow, completions gated on [`JobOutcome::Success`] (only
/// successful jobs feed `evidence_projection`, mirroring
/// [`crate::sim::Sim::step`]'s own gate), and one finalize unit per
/// `page_updates` entry this tick (`External` and drained-
/// `ScheduledBatch` runs can both finalize the same tick).
#[expect(
    clippy::cast_precision_loss,
    reason = "per-tick event counts are bounded well under 2^52 for any realistic sim run"
)]
fn apply_live_flow_rates(state: &mut AppState, events: &crate::sim::StepEvents) {
    use crate::sd::Flow;

    let scheduled_batch_rate = events
        .arrivals
        .iter()
        .filter(|(source, result)| {
            *source == JobSource::ScheduledBatch && *result == EnqueueResult::Accepted
        })
        .count() as f64;
    let external_rate = events
        .arrivals
        .iter()
        .filter(|(source, result)| {
            matches!(source, JobSource::External { .. }) && *result == EnqueueResult::Accepted
        })
        .count() as f64;
    let completion_rate = events
        .completions
        .iter()
        .filter(|(_, outcome)| *outcome == JobOutcome::Success)
        .count() as f64;
    let finalize_rate = events.page_updates.len() as f64;

    let ids = &state.spine_flows;
    state
        .scene
        .set_flow_rate(ids.scheduled_batch, Flow::Uniflow(scheduled_batch_rate));
    state
        .scene
        .set_flow_rate(ids.external, Flow::Uniflow(external_rate));
    state
        .scene
        .set_flow_rate(ids.initial_load, Flow::Uniflow(0.0));
    state
        .scene
        .set_flow_rate(ids.dequeue, state.queue_binding.outflow());
    state
        .scene
        .set_flow_rate(ids.completion, Flow::Uniflow(completion_rate));
    state
        .scene
        .set_flow_rate(ids.finalize, Flow::Uniflow(finalize_rate));
}

/// Refreshes every `#controls-hud` readout span from `state` — never
/// touches a button element, so listeners wired once in
/// [`wire_controls`] stay attached.
fn render_hud_readouts(document: &Document, state: &AppState) {
    render_rate_readouts(document, state.rates);
    set_text(
        document,
        "backend-label",
        backend_label(state.sim.durable_backend()),
    );
    set_text(
        document,
        "ws-permits-value",
        &format_bounded_level(
            state.sim.ws_permits_in_use(),
            state.sim.ws_max_connections(),
        ),
    );
    set_text(
        document,
        "github-budget-value",
        &format_bounded_level(
            state.sim.github_calls_used() as usize,
            state.sim.github_budget() as usize,
        ),
    );
    let outcome = state.sim.inventory_outcome();
    set_text(
        document,
        "inventory-value",
        &format!(
            "{} inventoried / {} reused / {} spawned",
            outcome.inventoried, outcome.reused_unchanged, outcome.jobs_spawned
        ),
    );
    set_text(
        document,
        "compression-ratio-value",
        &compression_ratio_label(
            state.sim.compressed_bytes_total(),
            state.sim.raw_bytes_total(),
        ),
    );
    set_text(
        document,
        "memo-value",
        &format!(
            "{} hits / {} rebuilds",
            state.sim.memo_hits(),
            state.sim.memo_rebuilds()
        ),
    );
    set_text(
        document,
        "batch-drained-value",
        &batch_drained_label(state.sim.batch_remaining()),
    );
}

/// The in-flight `BatchTracker` count expressed as a `"drained"` /
/// `"N in flight"` label, mirroring the pre-svs-05
/// `ConverterReadoutTemplate` batch-drained gate readout.
fn batch_drained_label(batch_remaining: usize) -> String {
    if batch_remaining == 0 {
        "drained".to_owned()
    } else {
        format!("{batch_remaining} in flight")
    }
}

/// `compressed / raw` expressed as a whole-percent label, `"n/a"` before
/// any page has landed (`raw_bytes_total == 0`) to avoid a divide-by-zero
/// display.
fn compression_ratio_label(compressed_bytes_total: usize, raw_bytes_total: usize) -> String {
    if raw_bytes_total == 0 {
        return "n/a".to_owned();
    }
    let percent = compressed_bytes_total * 100 / raw_bytes_total;
    format!("{percent}%")
}

fn backend_label(backend: PardosaBackend) -> &'static str {
    match backend {
        PardosaBackend::Pgno => "Pgno",
        PardosaBackend::Nats => "Nats",
    }
}

/// The one-time `#controls-hud` skeleton: warm-start/backend-toggle
/// buttons, rate +/- buttons, and the readout spans [`render_hud_readouts`]
/// targets by id every tick.
fn controls_markup(sim: &Sim) -> String {
    format!(
        r#"<div class="row">
  <button id="warm-start-btn">fire warm start</button>
  <span class="gauge">warm_start_from_baseline</span>
</div>
<div class="row">
  <button id="backend-toggle-btn">toggle Pgno/Nats</button>
  <span class="gauge">active PardosaBackend::<span id="backend-label">{backend_label}</span></span>
</div>
<div class="row">
  <button id="batch-rate-down">batch rate -</button>
  <span class="gauge">ScheduledBatch every <span id="batch-rate-value">{batch}</span> ticks</span>
  <button id="batch-rate-up">+</button>
  <button id="external-rate-down">external rate -</button>
  <span class="gauge">External every <span id="external-rate-value">{external}</span> ticks</span>
  <button id="external-rate-up">+</button>
</div>
<div class="row">
  <span class="gauge">ws permits <span id="ws-permits-value">0/0</span></span>
  <span class="gauge">github budget <span id="github-budget-value">0/0</span></span>
</div>
<div class="row">
  <span class="gauge">inventory sweep: <span id="inventory-value">none yet</span></span>
</div>
<div class="row">
  <span class="gauge">compression ratio <span id="compression-ratio-value">n/a</span></span>
  <span class="gauge">memo <span id="memo-value">0 hits / 0 rebuilds</span></span>
  <span class="gauge">batch <span id="batch-drained-value">drained</span></span>
</div>
"#,
        backend_label = backend_label(sim.durable_backend()),
        batch = DEFAULT_BATCH_PER_TICK,
        external = DEFAULT_EXTERNAL_PER_TICK,
    )
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
/// holding a labeled `div`. `Stock` nodes additionally get an inline
/// sparkline (`history`, already recorded this tick by [`tick`]) plus
/// their current numeric level — the node box scales together with
/// the rest of the SVG coordinate space rather than sitting in a
/// separate absolute-px HTML layer.
fn node_markup(built_scene: &Scene, node: &PlacedNode, history: Option<&LevelHistory>) -> String {
    let (x, y) = built_scene.node_origin(node);
    let grid = built_scene.grid();
    let class = node_class(node.geometry);
    let sparkline = history.map(stock_sparkline_markup).unwrap_or_default();
    format!(
        r#"<foreignObject x="{x}" y="{y}" width="{width}" height="{height}">
  <div xmlns="http://www.w3.org/1999/xhtml" class="{class}">
    <span class="scene-node-label">{label}</span>
    {sparkline}
  </div>
</foreignObject>
"#,
        width = grid.box_width,
        height = grid.box_height,
        label = node.label,
    )
}

/// A [`Stock`](crate::sd::Stock) node's inline sparkline: an SVG
/// polyline over `history`'s samples via
/// [`crate::sparkline::polyline_points`] (host-pure, tested), plus the
/// latest level as plain text — the renderer only turns already-scaled
/// `(x, y)` pairs into markup, computing no position itself.
fn stock_sparkline_markup(history: &LevelHistory) -> String {
    let samples: Vec<f64> = history.iter().collect();
    let latest = history.latest().unwrap_or(0.0);
    let points = crate::sparkline::polyline_points(&samples, SPARKLINE_WIDTH, SPARKLINE_HEIGHT);
    format!(
        r#"<div class="scene-node-metric">{latest:.0}</div>
    <svg class="scene-node-sparkline" viewBox="0 0 {SPARKLINE_WIDTH} {SPARKLINE_HEIGHT}" preserveAspectRatio="none">
      <polyline points="{points}" />
    </svg>"#
    )
}

/// The `BatchRemaining` stock's [`SweepPhase`](crate::sim::SweepPhase)
/// annotation: a plain non-SD badge positioned at that node's already-
/// placed origin (adr-fmt-vrycy hotspot (c) — `SweepPhase` is a
/// discrete control-state enum, never an `sd::Model` node, so this
/// shares no `scene-node-*` class with the wired SD nodes it sits
/// beside). Empty when the scene carries no node labeled
/// `"BatchRemaining"` (defensive — every `gh_report_scene` build has
/// exactly one).
fn sweep_phase_overlay_markup(built_scene: &Scene, sim: &Sim) -> String {
    let Some(node) = built_scene
        .nodes()
        .iter()
        .find(|node| node.label == "BatchRemaining")
    else {
        return String::new();
    };
    let (x, y) = built_scene.node_origin(node);
    let grid = built_scene.grid();
    format!(
        r#"<foreignObject x="{x}" y="{overlay_y}" width="{width}" height="24">
  <div xmlns="http://www.w3.org/1999/xhtml" class="scene-overlay-badge">SweepPhase (control-state, not SD): {phase:?}</div>
</foreignObject>
"#,
        overlay_y = y - 26.0,
        width = grid.box_width,
        phase = sim.sweep_phase(),
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
/// gauge, a belt-items layer, a nodes layer (each `Stock` node carrying
/// its inline sparkline + metric), and the `SweepPhase` overlay
/// annotation — all walked directly off `built_scene`/`sim`/`activity`
/// /`state.stock_histories`. The renderer's only geometry and motion
/// inputs are [`Scene::viewbox_dimensions`], [`Scene::node_origin`],
/// [`Scene::grid`], each [`Belt::path`], and the host-pure
/// [`belt_item_count`]/[`belt_item_phase`]/[`fill_fraction`]/
/// [`crate::sparkline::polyline_points`] functions.
fn scene_markup(
    built_scene: &Scene,
    sim: &Sim,
    tick_count: u64,
    activity: &BeltActivity,
    stock_histories: &[LevelHistory],
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
    let mut stock_index = 0usize;
    let nodes: String = built_scene
        .nodes()
        .iter()
        .map(|node| {
            let history = if matches!(node.geometry, NodeGeometry::Stock(..)) {
                let history = stock_histories.get(stock_index);
                stock_index += 1;
                history
            } else {
                None
            };
            node_markup(built_scene, node, history)
        })
        .collect();
    let overlay = sweep_phase_overlay_markup(built_scene, sim);
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
      <g class="scene-overlays">
{overlay}      </g>
    </svg>
  </div>
</section>
"##
    )
}
