//! `wasm32`-only reactive view: builds the queue-network DOM inside
//! `#app`, drives [`crate::sim::Sim`] frame-by-frame via [`tick`]
//! (called from `bootstrap.js`'s `setInterval`, not `web-sys`
//! `requestAnimationFrame` — kept out of scope to avoid depending on
//! workspace `web-sys` features beyond what `gh-report-web-client`
//! already declares), and animates packets colored by [`JobSource`].
//! Raw `web-sys` + leptos reactive primitives only — no `view!` macro,
//! mirrors `gh-report-web-client/src/dom.rs`.
//!
//! Renders gh-report's actual operational model (adr-fmt-223sd,
//! adr-fmt-t63uo) as causality, not a single conveyor belt: three
//! distinct trigger entry points (scheduled sweep, webhook, warm
//! start) feed a per-packet WRITE side, joined by the `BatchTracker`
//! barrier, gating a per-RUN READ side (`finalize_and_publish`), plus
//! a continuous SERVE path off the current `ArcSwap` generation.

use std::cell::RefCell;

use any_spawner::Executor;
use leptos::prelude::{Effect, Get, GetUntracked, Owner, RwSignal, Update};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::wasm_bindgen;
use web_sys::{Document, Element};

use crate::sim::{
    EnqueueResult, JobOutcome, JobSource, PageUpdateEvent, Sim, SimConfig, SweepPhase,
};

#[derive(Clone, Copy)]
struct Rates {
    batch_per_tick: u32,
    external_per_tick: u32,
}

impl Rates {
    const fn clamp(self) -> Self {
        Self {
            batch_per_tick: if self.batch_per_tick > 10 {
                10
            } else {
                self.batch_per_tick
            },
            external_per_tick: if self.external_per_tick > 10 {
                10
            } else {
                self.external_per_tick
            },
        }
    }
}

struct AppState {
    sim: Sim,
    rates: RwSignal<Rates>,
    tick_count: u64,
    warm_start_requested: bool,
}

thread_local! {
    static APP: RefCell<Option<AppState>> = const { RefCell::new(None) };
    static DOC: RefCell<Option<Document>> = const { RefCell::new(None) };
}

fn cached_document() -> Option<Document> {
    DOC.with(|cell| cell.borrow().clone())
}

fn source_color(source: JobSource) -> &'static str {
    match source {
        JobSource::ScheduledBatch => "#3b82f6",
        JobSource::External { .. } => "#22c55e",
        JobSource::InitialLoad => "#94a3b8",
    }
}

const WRITE_LANE_END: &str = "100%";
const WRITE_DURATION_MS: u32 = 1400;
const FAILURE_LANE_END: &str = "45%";
const FAILURE_DURATION_MS: u32 = 700;
const READ_LANE_END: &str = "100%";
const READ_PULSE_DURATION_MS: u32 = 900;
const WARMSTART_DURATION_MS: u32 = 1100;
const MAX_LANE_PACKETS: u32 = 40;

/// Curve `spawn_collection_loop` &rarr; `WorkQueue` (fan-in leg one).
const PATH_CONVERGE_SCHEDULED: &str = "M180,95 C300,95 300,180 280,180";
/// Curve `webhook_handler` &rarr; `WorkQueue` (fan-in leg two).
const PATH_CONVERGE_WEBHOOK: &str = "M180,265 C300,265 300,180 280,180";
/// Shared write spine: `WorkQueue` &rarr; `worker_loop` &rarr;
/// `EvidenceProjectionEvent` stream &rarr; `EvidenceProjection`.
const PATH_WRITE_SPINE: &str = "M420,180 C450,140 500,140 555,180 \
     C610,220 660,220 690,180 C720,140 780,140 830,180 C860,210 890,210 910,180";
/// `BatchTracker` gate segment: `EvidenceProjection` &rarr;
/// `finalize_and_publish`.
const PATH_GATE: &str = "M1050,180 C1000,220 950,260 925,330";
/// Read chain: `finalize_and_publish` &rarr; `build_cached_pages` &rarr;
/// `commit_cached_pages`.
const PATH_READ_CHAIN: &str = "M975,380 Q1000,340 1050,380 Q1100,420 1130,380";
/// `warm_start_from_baseline` bypass, routed around the write side and
/// barrier straight into the read chain.
const PATH_WARMSTART_BYPASS: &str = "M180,445 C450,445 450,620 700,620 C820,620 850,500 900,410";
/// Continuous serve branch off `commit_cached_pages` / `ArcSwap`.
const PATH_SERVE_BRANCH: &str = "M1200,415 C1200,460 1200,460 1200,505";
/// `PageUpdateEvent` WS loop back from the serve branch.
const PATH_SERVE_LOOP: &str = "M1270,540 C1279,480 1279,420 1265,415";

fn sweep_phase_label(phase: SweepPhase) -> &'static str {
    match phase {
        SweepPhase::Init => "Init",
        SweepPhase::Resumed => "Resumed",
        SweepPhase::BaselineReused => "BaselineReused",
        SweepPhase::AwaitingBatch => "AwaitingBatch",
        SweepPhase::BatchDrained => "BatchDrained",
        SweepPhase::Completed => "Completed",
        SweepPhase::Failed { .. } => "Failed",
    }
}

#[wasm_bindgen(start)]
pub fn start() {
    let _ = Executor::init_wasm_bindgen();
    let owner = Owner::new();
    owner.set();
    std::mem::forget(owner);

    let Some(document) = web_sys::window().and_then(|window| window.document()) else {
        return;
    };
    let Some(root) = document.get_element_by_id("app") else {
        return;
    };
    DOC.with(|cell| *cell.borrow_mut() = Some(document.clone()));

    let rates = RwSignal::new(Rates {
        batch_per_tick: 1,
        external_per_tick: 1,
    });

    build_layout(&document, &root, rates);

    let sim = Sim::new(SimConfig::default(), 7);
    APP.with(|cell| {
        *cell.borrow_mut() = Some(AppState {
            sim,
            rates,
            tick_count: 0,
            warm_start_requested: false,
        });
    });

    wire_warm_start_button(&document);

    Effect::new(move |_| {
        let current = rates.get();
        if let Some(document) = cached_document() {
            set_text(
                &document,
                "batch-rate-value",
                &current.batch_per_tick.to_string(),
            );
            set_text(
                &document,
                "external-rate-value",
                &current.external_per_tick.to_string(),
            );
        }
    });
}

/// Advance the simulation by one tick and re-render gauges + packets.
///
/// Invoked from `bootstrap.js` on a `setInterval` cadence — the
/// animation clock lives in JS, keeping this crate's `web-sys`
/// dependency limited to the DOM-manipulation surface already declared
/// for `gh-report-web-client` (no `requestAnimationFrame`/`Performance`
/// features required).
#[wasm_bindgen]
pub fn tick() {
    let Some(document) = cached_document() else {
        return;
    };

    APP.with(|cell| {
        let mut borrowed = cell.borrow_mut();
        let Some(state) = borrowed.as_mut() else {
            return;
        };

        if state.warm_start_requested {
            state.warm_start_requested = false;
            let update = state.sim.warm_start();
            if let Some(packet_layer) = document.get_element_by_id("packet-layer") {
                spawn_transit_packet(
                    &document,
                    &packet_layer,
                    &PacketSpec {
                        class: "packet packet-warmstart",
                        color: source_color(JobSource::InitialLoad),
                        path_d: PATH_WARMSTART_BYPASS,
                        start: "0%",
                        end: "100%",
                        duration_ms: WARMSTART_DURATION_MS,
                    },
                );
            }
            render_read_pulse(&document, update, false);
        }

        let rates = state.rates.get_untracked();
        let batch_arrival = state
            .tick_count
            .is_multiple_of(u64::from(rates.batch_per_tick.max(1)));
        let external_arrival = state
            .tick_count
            .is_multiple_of(u64::from(rates.external_per_tick.max(1)));
        let events = state.sim.step(batch_arrival, external_arrival);
        state.tick_count += 1;

        render_gauges(&document, &state.sim);
        render_events(&document, &events.arrivals, &events.completions);
        for update in &events.page_updates {
            render_read_pulse(&document, *update, true);
        }
    });
}

fn render_gauges(document: &Document, sim: &Sim) {
    set_text(document, "queue-depth", &sim.queue_depth().to_string());
    set_text(
        document,
        "queue-capacity",
        &sim.queue_capacity().to_string(),
    );
    set_text(document, "in-flight", &sim.in_flight().to_string());
    set_text(document, "worker-count", &sim.worker_count().to_string());
    set_text(
        document,
        "batch-remaining",
        &sim.batch_remaining().to_string(),
    );
    set_text(document, "served-pages", &sim.served_pages().to_string());
    let metrics = sim.metrics();
    set_text(
        document,
        "queue-full-count",
        &metrics.queue_full.to_string(),
    );
    set_text(
        document,
        "deduplicated-count",
        &metrics.deduplicated.to_string(),
    );
    set_text(document, "failure-count", &metrics.failures.to_string());
    set_text(
        document,
        "events-written",
        &sim.events_written().to_string(),
    );
    set_text(
        document,
        "repos-captured",
        &sim.repositories_captured().to_string(),
    );
    set_text(document, "memo-hits", &sim.memo_hits().to_string());
    set_text(document, "memo-rebuilds", &sim.memo_rebuilds().to_string());
    set_text(
        document,
        "compression-ratio",
        &compression_ratio_display(sim),
    );
    set_text(
        document,
        "arcswap-generation",
        &sim.arcswap_generation().to_string(),
    );
    set_text(
        document,
        "stage-queue-fill",
        &format!("{}/{}", sim.queue_depth(), sim.queue_capacity()),
    );
    set_text(
        document,
        "stage-stream-fill",
        &sim.events_written().to_string(),
    );
    set_text(
        document,
        "stage-projection-fill",
        &sim.repositories_captured().to_string(),
    );
    set_text(
        document,
        "stage-cache-fill",
        &sim.served_pages().to_string(),
    );
    set_text(
        document,
        "sweep-phase",
        sweep_phase_label(sim.sweep_phase()),
    );
    set_text(
        document,
        "cache-fallback-gen",
        &sim.cache_fallback().to_string(),
    );
    set_text(
        document,
        "worker-executions",
        &sim.worker_executions().to_string(),
    );
}

fn compression_ratio_display(sim: &Sim) -> String {
    let raw = sim.raw_bytes_total();
    if raw == 0 {
        return "n/a".to_string();
    }
    let percent = sim.compressed_bytes_total() * 100 / raw;
    format!("{percent}%")
}

/// Bundles a packet's visual + motion parameters so
/// [`spawn_transit_packet`] stays under clippy's argument-count bar.
struct PacketSpec<'a> {
    class: &'a str,
    color: &'a str,
    path_d: &'a str,
    start: &'a str,
    end: &'a str,
    duration_ms: u32,
}

fn render_events(
    document: &Document,
    arrivals: &[(JobSource, EnqueueResult)],
    completions: &[(JobSource, JobOutcome)],
) {
    let Some(packet_layer) = document.get_element_by_id("packet-layer") else {
        return;
    };
    for (source, result) in arrivals {
        let class = match result {
            EnqueueResult::Accepted => "packet packet-accepted",
            EnqueueResult::Deduplicated => "packet packet-deduplicated",
            EnqueueResult::QueueFull => "packet packet-dropped",
        };
        spawn_transit_packet(
            document,
            &packet_layer,
            &PacketSpec {
                class,
                color: source_color(*source),
                path_d: converge_path_for(*source),
                start: "0%",
                end: "100%",
                duration_ms: WRITE_DURATION_MS / 2,
            },
        );
    }

    for (source, outcome) in completions {
        let spec = match outcome {
            JobOutcome::Success => PacketSpec {
                class: "packet packet-success",
                color: source_color(*source),
                path_d: PATH_WRITE_SPINE,
                start: "0%",
                end: WRITE_LANE_END,
                duration_ms: WRITE_DURATION_MS,
            },
            JobOutcome::Failure => PacketSpec {
                class: "packet packet-failure",
                color: source_color(*source),
                path_d: PATH_WRITE_SPINE,
                start: "0%",
                end: FAILURE_LANE_END,
                duration_ms: FAILURE_DURATION_MS,
            },
        };
        spawn_transit_packet(document, &packet_layer, &spec);
    }
}

/// The converge leg a [`JobSource`] fans into `WorkQueue` on.
/// [`JobSource::InitialLoad`] never arrives via this fan-in (it rides
/// [`PATH_WARMSTART_BYPASS`] instead); the scheduled leg is a harmless
/// fallback for that unreachable-in-practice arrival case.
const fn converge_path_for(source: JobSource) -> &'static str {
    match source {
        JobSource::External { .. } => PATH_CONVERGE_WEBHOOK,
        JobSource::ScheduledBatch | JobSource::InitialLoad => PATH_CONVERGE_SCHEDULED,
    }
}

/// Pulses the READ chain once per [`PageUpdateEvent`] —
/// `finalize_and_publish` firing per RUN, never per packet. `gated`
/// flashes the `BatchTracker` gate glyph: true for the scheduled-run
/// path (gate already enforced `remaining == 0` upstream in
/// [`crate::sim::Sim::step`]), false for the warm-start bypass, which
/// never touches the gate.
fn render_read_pulse(document: &Document, update: PageUpdateEvent, gated: bool) {
    let Some(packet_layer) = document.get_element_by_id("packet-layer") else {
        return;
    };
    spawn_transit_packet(
        document,
        &packet_layer,
        &PacketSpec {
            class: "packet packet-page-update",
            color: "#f59e0b",
            path_d: PATH_READ_CHAIN,
            start: "0%",
            end: READ_LANE_END,
            duration_ms: READ_PULSE_DURATION_MS,
        },
    );
    if gated {
        flash_gate(document);
    }
    set_text(
        document,
        "arcswap-generation",
        &update.generation.to_string(),
    );
}

/// Restarts the `gate-flash` CSS animation on the `BatchTracker` glyph
/// by clearing then re-applying it, forcing a reflow in between so the
/// keyframes restart even when the previous flash is still fading.
fn flash_gate(document: &Document) {
    let Some(element) = document.get_element_by_id("gate-glyph") else {
        return;
    };
    let Ok(html_element) = element.dyn_into::<web_sys::HtmlElement>() else {
        return;
    };
    let style = html_element.style();
    style.set_property("animation", "none").ok();
    let _forced_reflow = html_element.offset_width();
    style
        .set_property("animation", "gate-flash 900ms ease-out")
        .ok();
}

fn wire_warm_start_button(document: &Document) {
    let Some(element) = document.get_element_by_id("warm-start-btn") else {
        return;
    };
    let closure = wasm_bindgen::closure::Closure::<dyn FnMut(web_sys::Event)>::new(move |_event| {
        APP.with(|cell| {
            if let Some(state) = cell.borrow_mut().as_mut() {
                state.warm_start_requested = true;
            }
        });
    });
    let _ignored =
        element.add_event_listener_with_callback("click", closure.as_ref().unchecked_ref());
    closure.forget();
}

fn spawn_transit_packet(document: &Document, layer: &Element, spec: &PacketSpec<'_>) {
    let Ok(packet) = document.create_element("div") else {
        return;
    };
    packet.set_attribute("class", spec.class).ok();
    if let Ok(html_packet) = packet.clone().dyn_into::<web_sys::HtmlElement>() {
        let style = html_packet.style();
        style.set_property("background-color", spec.color).ok();
        style
            .set_property("offset-path", &format!("path('{}')", spec.path_d))
            .ok();
        style.set_property("--transit-start", spec.start).ok();
        style.set_property("--transit-end", spec.end).ok();
        style
            .set_property("animation-duration", &format!("{}ms", spec.duration_ms))
            .ok();
    }
    layer.append_child(&packet).ok();
    prune_lane(layer);
}

fn prune_lane(lane: &Element) {
    if lane.child_element_count() > MAX_LANE_PACKETS
        && let Some(first) = lane.first_element_child()
    {
        lane.remove_child(&first).ok();
    }
}

fn set_text(document: &Document, id: &str, value: &str) {
    if let Some(element) = document.get_element_by_id(id) {
        element.set_text_content(Some(value));
    }
}

fn build_layout(document: &Document, root: &Element, rates: RwSignal<Rates>) {
    root.set_inner_html(&graph_markup());

    wire_rate_button(document, "batch-rate-down", rates, RateField::Batch, -1);
    wire_rate_button(document, "batch-rate-up", rates, RateField::Batch, 1);
    wire_rate_button(
        document,
        "external-rate-down",
        rates,
        RateField::External,
        -1,
    );
    wire_rate_button(document, "external-rate-up", rates, RateField::External, 1);
}

/// The full branched flow-graph markup: SVG curved edges layer plus
/// absolutely-positioned HTML node boxes, sharing the same 1280x620
/// coordinate space so [`spawn_transit_packet`]'s `offset-path` lines
/// up with the drawn edges.
fn graph_markup() -> String {
    format!(
        r"
<section class='queue-viz'>
  <h1>gh-report queue network &mdash; branched causal flow graph</h1>
  <div class='row controls'>
    <button id='batch-rate-down'>batch rate -</button>
    <span>ScheduledBatch every <span id='batch-rate-value'>1</span> ticks</span>
    <button id='batch-rate-up'>+</button>
    <button id='external-rate-down'>external rate -</button>
    <span>External every <span id='external-rate-value'>1</span> ticks</span>
    <button id='external-rate-up'>+</button>
  </div>

  <div class='graph-canvas'>
    <svg class='graph-svg' viewBox='0 0 1280 620' preserveAspectRatio='xMidYMid meet'>
      <defs>
        <marker id='arrow' viewBox='0 0 10 10' refX='9' refY='5' markerWidth='7' markerHeight='7' orient='auto-start-reverse'>
          <path d='M0,0 L10,5 L0,10 z' fill='#94a3b8' />
        </marker>
      </defs>
      <path class='edge edge-converge' d='{PATH_CONVERGE_SCHEDULED}' marker-end='url(#arrow)' />
      <path class='edge edge-converge' d='{PATH_CONVERGE_WEBHOOK}' marker-end='url(#arrow)' />
      <path class='edge edge-spine' d='{PATH_WRITE_SPINE}' marker-end='url(#arrow)' />
      <path class='edge edge-gate' d='{PATH_GATE}' marker-end='url(#arrow)' />
      <path class='edge edge-read' d='{PATH_READ_CHAIN}' marker-end='url(#arrow)' />
      <path class='edge edge-warmstart' d='{PATH_WARMSTART_BYPASS}' marker-end='url(#arrow)' />
      <path class='edge edge-serve' d='{PATH_SERVE_BRANCH}' marker-end='url(#arrow)' />
      <path class='edge edge-serve-loop' d='{PATH_SERVE_LOOP}' marker-end='url(#arrow)' />
    </svg>
    <div id='packet-layer' class='packet-layer'></div>
{}
  </div>

  <div class='row legend'>
    <div class='gauge'>WorkQueue <span id='queue-depth'>0</span>/<span id='queue-capacity'>0</span></div>
    <div class='gauge'>in-flight <span id='in-flight'>0</span>/<span id='worker-count'>0</span></div>
    <div class='gauge'>QueueFull <span id='queue-full-count'>0</span></div>
    <div class='gauge'>Deduplicated <span id='deduplicated-count'>0</span></div>
    <div class='gauge'>failures <span id='failure-count'>0</span></div>
  </div>
  <div class='row legend'>
    <div class='gauge'>events written <span id='events-written'>0</span></div>
    <div class='gauge'>repos captured <span id='repos-captured'>0</span></div>
    <div class='gauge'>memo hits <span id='memo-hits'>0</span></div>
    <div class='gauge'>memo rebuilds <span id='memo-rebuilds'>0</span></div>
    <div class='gauge'>compression <span id='compression-ratio'>n/a</span></div>
  </div>
</section>
",
        graph_nodes_markup(),
    )
}

/// Node boxes only (triggers, write spine, gate, read chain, serve),
/// split out of [`graph_markup`] purely to stay under clippy's
/// function-length bar.
fn graph_nodes_markup() -> &'static str {
    r"
    <div class='node node-trigger node-scheduled' style='left:95px;top:95px'>
      spawn_collection_loop
      <span class='stage-note'>SweepPhase: <span id='sweep-phase'>Completed</span></span>
      <span class='stage-note'>ScheduledBatch (burst)</span>
    </div>
    <div class='node node-trigger node-webhook' style='left:95px;top:265px'>
      webhook_handler
      <span class='stage-note'>JobSource::External&lbrace;id,kind&rbrace;</span>
    </div>
    <div class='node node-trigger node-warmstart' style='left:95px;top:445px'>
      warm_start_from_baseline
      <span class='stage-note'>bypass &rarr; read chain</span>
      <button id='warm-start-btn'>fire warm start</button>
    </div>

    <div class='node node-store node-queue' style='left:350px;top:180px'>
      WorkQueue
      <span class='stage-note'>depth <span id='stage-queue-fill'>0/0</span></span>
    </div>
    <div class='node node-work node-worker' style='left:555px;top:180px'>
      worker_loop / LiveEvaluator::evaluate
      <span class='stage-note'>executions <span id='worker-executions'>0</span></span>
    </div>
    <div class='node node-store node-eventstream' style='left:775px;top:180px'>
      EvidenceProjectionEvent stream
      <span class='stage-note'>events <span id='stage-stream-fill'>0</span></span>
    </div>
    <div class='node node-store node-projection' style='left:980px;top:180px'>
      EvidenceProjection
      <span class='stage-note'>repos <span id='stage-projection-fill'>0</span></span>
    </div>

    <div id='gate-glyph' class='gate-glyph' style='left:950px;top:280px'>
      &#9670;
      <span class='stage-note'>BatchTracker rem <span id='batch-remaining'>0</span></span>
    </div>

    <div class='node node-work node-finalize' style='left:900px;top:380px'>
      finalize_and_publish
      <span class='stage-note'>per RUN</span>
    </div>
    <div class='node node-work node-buildcache' style='left:1050px;top:380px'>
      build_cached_pages
      <span class='stage-note'>memo</span>
    </div>
    <div class='node node-work node-commit' style='left:1200px;top:380px'>
      commit_cached_pages
      <span class='stage-note'>ArcSwap gen <span id='arcswap-generation'>0</span></span>
    </div>

    <div class='node node-serve node-served' style='left:1200px;top:540px'>
      cache_fallback &rarr; served pages
      <span class='stage-note'>gen <span id='cache-fallback-gen'>0</span></span>
      <span class='stage-note'>served <span id='served-pages'>0</span></span>
      <span class='stage-note stage-hidden'>cache fill <span id='stage-cache-fill'>0</span></span>
    </div>
"
}

#[derive(Clone, Copy)]
enum RateField {
    Batch,
    External,
}

fn wire_rate_button(
    document: &Document,
    id: &str,
    rates: RwSignal<Rates>,
    field: RateField,
    delta: i32,
) {
    let Some(element) = document.get_element_by_id(id) else {
        return;
    };
    let closure = wasm_bindgen::closure::Closure::<dyn FnMut(web_sys::Event)>::new(move |_event| {
        rates.update(|current| {
            let updated = match field {
                RateField::Batch => Rates {
                    batch_per_tick: apply_delta(current.batch_per_tick, delta),
                    ..*current
                },
                RateField::External => Rates {
                    external_per_tick: apply_delta(current.external_per_tick, delta),
                    ..*current
                },
            };
            *current = updated.clamp();
        });
    });
    let _ignored =
        element.add_event_listener_with_callback("click", closure.as_ref().unchecked_ref());
    closure.forget();
}

fn apply_delta(value: u32, delta: i32) -> u32 {
    if delta.is_negative() {
        value.saturating_sub(delta.unsigned_abs()).max(1)
    } else {
        value.saturating_add(delta.unsigned_abs())
    }
}
