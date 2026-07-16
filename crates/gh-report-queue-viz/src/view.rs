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
const MAX_LANE_PACKETS: u32 = 40;

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
        if let Some(document) = web_sys::window().and_then(|window| window.document()) {
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
    let Some(document) = web_sys::window().and_then(|window| window.document()) else {
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
            render_read_pulse(&document, update);
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
            render_read_pulse(&document, *update);
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

fn render_events(
    document: &Document,
    arrivals: &[(JobSource, EnqueueResult)],
    completions: &[(JobSource, JobOutcome)],
) {
    let Some(write_lane) = document.get_element_by_id("write-lane") else {
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
            &write_lane,
            class,
            source_color(*source),
            "0%",
            "20%",
            WRITE_DURATION_MS / 2,
        );
    }

    for (source, outcome) in completions {
        match outcome {
            JobOutcome::Success => spawn_transit_packet(
                document,
                &write_lane,
                "packet packet-success",
                source_color(*source),
                "20%",
                WRITE_LANE_END,
                WRITE_DURATION_MS,
            ),
            JobOutcome::Failure => spawn_transit_packet(
                document,
                &write_lane,
                "packet packet-failure",
                source_color(*source),
                "20%",
                FAILURE_LANE_END,
                FAILURE_DURATION_MS,
            ),
        }
    }
}

/// Pulses the READ-side lane once per [`PageUpdateEvent`] —
/// `finalize_and_publish` firing per RUN, never per packet.
fn render_read_pulse(document: &Document, update: PageUpdateEvent) {
    let Some(read_lane) = document.get_element_by_id("read-lane") else {
        return;
    };
    spawn_transit_packet(
        document,
        &read_lane,
        "packet packet-page-update",
        "#f59e0b",
        "0%",
        READ_LANE_END,
        READ_PULSE_DURATION_MS,
    );
    set_text(
        document,
        "arcswap-generation",
        &update.generation.to_string(),
    );
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

fn spawn_transit_packet(
    document: &Document,
    lane: &Element,
    class: &str,
    color: &str,
    start: &str,
    end: &str,
    duration_ms: u32,
) {
    let Ok(packet) = document.create_element("div") else {
        return;
    };
    packet.set_attribute("class", class).ok();
    if let Ok(html_packet) = packet.clone().dyn_into::<web_sys::HtmlElement>() {
        let style = html_packet.style();
        style.set_property("background-color", color).ok();
        style.set_property("--transit-start", start).ok();
        style.set_property("--transit-end", end).ok();
        style
            .set_property("animation-duration", &format!("{duration_ms}ms"))
            .ok();
    }
    lane.append_child(&packet).ok();
    prune_lane(lane);
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
    root.set_inner_html(
        r"
<section class='queue-viz'>
  <h1>gh-report queue network — write &rarr; barrier &rarr; read, serve continuous</h1>
  <div class='row triggers'>
    <div class='trigger scheduled'>
      TRIGGER 1: spawn_collection_loop
      <span class='stage-note'>SweepPhase: <span id='sweep-phase'>Completed</span></span>
      <span class='stage-note'>enqueue_batch &rarr; JobSource::ScheduledBatch</span>
    </div>
    <div class='trigger webhook'>
      TRIGGER 2: webhook_handler
      <span class='stage-note'>JobSource::External{id,kind}</span>
    </div>
    <div class='trigger warmstart'>
      TRIGGER 3: warm_start_from_baseline
      <span class='stage-note'>bypasses queue/workers</span>
      <button id='warm-start-btn'>fire warm start</button>
    </div>
  </div>
  <div class='row controls'>
    <button id='batch-rate-down'>batch rate -</button>
    <span>ScheduledBatch every <span id='batch-rate-value'>1</span> ticks</span>
    <button id='batch-rate-up'>+</button>
    <button id='external-rate-down'>external rate -</button>
    <span>External every <span id='external-rate-value'>1</span> ticks</span>
    <button id='external-rate-up'>+</button>
  </div>

  <h2 class='split-label'>WRITE side (per packet)</h2>
  <div class='pipeline'>
    <div class='stage stage-store'>WorkQueue<span class='stage-note'>depth <span id='stage-queue-fill'>0/0</span></span></div>
    <div class='stage stage-work'>worker_loop / LiveEvaluator::evaluate<span class='stage-note'>16x GitHub query</span></div>
    <div class='stage stage-store'>EvidenceProjectionEvent stream<span class='stage-note'>record_repo events <span id='stage-stream-fill'>0</span></span></div>
    <div class='stage stage-store'>EvidenceProjection<span class='stage-note'>repos captured <span id='stage-projection-fill'>0</span></span></div>
    <div id='write-lane' class='lane lane-transit'></div>
  </div>

  <h2 class='split-label'>BARRIER — BatchTracker (scheduled runs only)</h2>
  <div class='row'>
    <div class='gauge'>BatchTracker remaining <span id='batch-remaining'>0</span></div>
    <div class='gauge'>worker executions <span id='worker-executions'>0</span></div>
  </div>

  <h2 class='split-label'>READ side (per RUN, gated on BatchTracker == 0)</h2>
  <div class='pipeline'>
    <div class='stage stage-work'>finalize_and_publish<span class='stage-note'>fires once per run</span></div>
    <div class='stage stage-work'>build_cached_pages<span class='stage-note'>memo</span></div>
    <div class='stage stage-work'>commit_cached_pages<span class='stage-note'>ArcSwap swap + PageUpdateEvent</span></div>
    <div id='read-lane' class='lane lane-transit'></div>
  </div>

  <h2 class='split-label'>SERVE (continuous, independent of runs)</h2>
  <div class='row'>
    <div class='gauge'>cache_fallback gen <span id='cache-fallback-gen'>0</span></div>
    <div class='gauge'>ArcSwap gen <span id='arcswap-generation'>0</span></div>
    <div class='gauge'>served pages (WS PageUpdateEvent) <span id='served-pages'>0</span></div>
  </div>

  <div class='row'>
    <div class='gauge'>WorkQueue <span id='queue-depth'>0</span>/<span id='queue-capacity'>0</span></div>
    <div class='gauge'>in-flight <span id='in-flight'>0</span>/<span id='worker-count'>0</span></div>
    <div class='gauge'>QueueFull <span id='queue-full-count'>0</span></div>
    <div class='gauge'>Deduplicated <span id='deduplicated-count'>0</span></div>
  </div>
  <div class='row'>
    <div class='gauge'>failures <span id='failure-count'>0</span></div>
    <div class='gauge'>events written <span id='events-written'>0</span></div>
    <div class='gauge'>repos captured <span id='repos-captured'>0</span></div>
    <div class='gauge'>memo hits <span id='memo-hits'>0</span></div>
    <div class='gauge'>memo rebuilds <span id='memo-rebuilds'>0</span></div>
    <div class='gauge'>compression <span id='compression-ratio'>n/a</span></div>
  </div>
</section>
",
    );

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
