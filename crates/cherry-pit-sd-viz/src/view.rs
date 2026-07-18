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

use crate::binding::{QueueStockBinding, ReadoutStock};
use crate::components::{CloudBoundaryMarkerTemplate, ConverterReadoutTemplate, StockTemplate};
use crate::layout::{self, StockKind};
use crate::overlay::SweepPhaseBadge;
use crate::sd::Terminal;
use crate::sim::{
    EnqueueResult, InventoryOutcome, JobOutcome, JobSource, PageUpdateEvent, PardosaBackend, Sim,
    SimConfig, SweepPhase, UpdatedAt,
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
    queue_binding: QueueStockBinding,
    queue_component: Option<StockTemplate>,
    in_flight_component: Option<StockTemplate>,
    in_flight_readout: ReadoutStock,
    batch_component: Option<StockTemplate>,
    batch_readout: ReadoutStock,
    batch_drained_component: Option<ConverterReadoutTemplate>,
    projection_component: Option<StockTemplate>,
    projection_readout: ReadoutStock,
    generation_component: Option<StockTemplate>,
    generation_readout: ReadoutStock,
    served_pages_component: Option<StockTemplate>,
    served_pages_readout: ReadoutStock,
    events_written_component: Option<StockTemplate>,
    events_written_readout: ReadoutStock,
    compression_component: Option<ConverterReadoutTemplate>,
    github_cloud: Option<CloudBoundaryMarkerTemplate>,
    web_clients_cloud: Option<CloudBoundaryMarkerTemplate>,
    durable_cloud: Option<CloudBoundaryMarkerTemplate>,
    sweep_phase_overlay: Option<SweepPhaseBadge>,
    rates: RwSignal<Rates>,
    tick_count: u64,
    warm_start_requested: bool,
    backend_toggle_requested: bool,
    last_worker_executions: u64,
    inventory_epoch: u64,
}

thread_local! {
    static APP: RefCell<Option<AppState>> = const { RefCell::new(None) };
    static DOC: RefCell<Option<Document>> = const { RefCell::new(None) };
}

fn cached_document() -> Option<Document> {
    DOC.with(|cell| cell.borrow().clone())
}

pub(crate) fn source_color(source: JobSource) -> &'static str {
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

/// Curve `spawn_collection_loop` &rarr; `WorkQueue` (fan-in leg one),
/// via the inventory listing + `should_reuse` gate.
const PATH_CONVERGE_SCHEDULED: &str = "M180,120 C300,120 300,180 280,180";
/// Curve `webhook_handler` &rarr; `WorkQueue` (fan-in leg two).
const PATH_CONVERGE_WEBHOOK: &str = "M180,300 C300,300 300,180 280,180";
/// Shared write spine: `WorkQueue` &rarr; `worker_loop` &rarr;
/// `EvidenceProjectionEvent` stream &rarr; `EvidenceProjection`.
const PATH_WRITE_SPINE: &str = "M420,180 C450,140 500,140 555,180 \
     C610,220 660,220 690,180 C720,140 780,140 830,180 C860,210 890,210 910,180";
/// `BatchTracker` gate segment: `EvidenceProjection` &rarr;
/// `finalize_and_publish`.
const PATH_GATE: &str = "M1050,180 C1000,240 950,300 925,390";
/// Read chain: `finalize_and_publish` &rarr; `build_cached_pages` &rarr;
/// `commit_cached_pages`.
const PATH_READ_CHAIN: &str = "M975,440 Q1000,400 1050,440 Q1100,480 1130,440";
/// `warm_start_from_baseline` bypass, routed around the write side and
/// barrier straight into the read chain.
const PATH_WARMSTART_BYPASS: &str = "M180,470 C450,500 450,660 700,660 C820,660 850,540 900,470";
/// Continuous serve branch off `commit_cached_pages` / `ArcSwap`.
const PATH_SERVE_BRANCH: &str = "M1200,475 C1200,520 1200,520 1200,565";
/// `PageUpdateEvent` WS loop back from the serve branch.
const PATH_SERVE_LOOP: &str = "M1270,600 C1279,540 1279,480 1265,475";
/// `github.com` (ABOVE `worker_loop`) &rarr; `webhook_handler`: GitHub
/// PUSHES webhook deliveries in.
const PATH_GITHUB_PUSH: &str = "M480,70 C300,70 160,180 155,270";
/// `github.com` &rarr; `spawn_collection_loop`: the inventory LISTING
/// (`build_inventory_from_api`, `GET /orgs/{org}/repos?type=all`).
const PATH_GITHUB_INVENTORY: &str = "M480,60 C300,40 160,60 130,100";
/// `worker_loop`/`LiveEvaluator::evaluate` &rarr; `github.com` (directly
/// above): the worker PULLS `repo_details` + the six concurrent
/// collector calls, gated by `RateLimitState`/`BudgetGate`.
const PATH_GITHUB_PULL: &str = "M555,155 C555,120 555,110 555,90";
/// `cache_fallback` &rarr; web clients: the per-request HTTP serve
/// edge.
const PATH_CLIENTS_HTTP: &str = "M1200,625 C1200,660 1200,690 1200,720";
/// `commit_cached_pages` &rarr; web clients: the per-RUN
/// `PageUpdateEvent` WS broadcast fan-out.
const PATH_CLIENTS_WS: &str = "M1155,475 C1080,560 1080,660 1140,715";
/// `NativeStore::record` facade &rarr; local `.pgno` file store
/// (`events.pgno`) — the DEFAULT active backend.
const PATH_BACKEND_PGNO: &str = "M775,215 C740,260 700,290 690,320";
/// `NativeStore::record` facade &rarr; `NATS` `JetStream`
/// (`JetStreamHandle::append`) — the alternate backend, dimmed until
/// `PardosaBackend::Nats` is selected.
const PATH_BACKEND_NATS: &str = "M810,215 C850,260 880,290 890,320";
const GITHUB_PUSH_DURATION_MS: u32 = 900;
const GITHUB_PULL_DURATION_MS: u32 = 1600;
const GITHUB_INVENTORY_DURATION_MS: u32 = 1300;
const CLIENT_WS_DURATION_MS: u32 = 1000;

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

fn mount_stock(
    document: &Document,
    mount_id: &str,
    title: &str,
    kind: StockKind,
) -> Option<StockTemplate> {
    document
        .get_element_by_id(mount_id)
        .and_then(|container| StockTemplate::mount(&container, title, kind))
}

fn mount_converter(
    document: &Document,
    mount_id: &str,
    label: &str,
) -> Option<ConverterReadoutTemplate> {
    document
        .get_element_by_id(mount_id)
        .and_then(|container| ConverterReadoutTemplate::mount(&container, label))
}

fn mount_cloud(
    document: &Document,
    mount_id: &str,
    label: &str,
    terminal: Terminal,
) -> Option<CloudBoundaryMarkerTemplate> {
    document
        .get_element_by_id(mount_id)
        .and_then(|container| CloudBoundaryMarkerTemplate::mount(&container, label, terminal))
}

/// Mounts every SD component + the non-SD `SweepPhase` overlay and
/// builds the initial [`AppState`] — split out of [`start`] purely to
/// stay under clippy's function-length bar.
fn mount_app_state(document: &Document, rates: RwSignal<Rates>) -> AppState {
    let sim = Sim::new(SimConfig::default(), 7);
    let queue_binding = QueueStockBinding::new(&sim);
    let queue_component = mount_stock(
        document,
        "queue-stock-mount",
        "WorkQueue",
        StockKind::Standard,
    );
    let in_flight_component = mount_stock(
        document,
        "in-flight-stock-mount",
        "in_flight",
        StockKind::Bounded,
    );
    let batch_component = mount_stock(
        document,
        "batch-stock-mount",
        "BatchTracker",
        StockKind::Accumulator,
    );
    let batch_drained_component =
        mount_converter(document, "batch-drained-mount", "barrier_drained");
    let projection_component = mount_stock(
        document,
        "projection-stock-mount",
        "EvidenceProjection",
        StockKind::Accumulator,
    );
    let generation_component = mount_stock(
        document,
        "generation-stock-mount",
        "ArcSwap generation",
        StockKind::Monotonic,
    );
    let served_pages_component = mount_stock(
        document,
        "served-pages-stock-mount",
        "served_pages",
        StockKind::Monotonic,
    );
    let events_written_component = mount_stock(
        document,
        "events-written-stock-mount",
        "events_written",
        StockKind::Monotonic,
    );
    let compression_component =
        mount_converter(document, "compression-converter-mount", "compression ratio");
    let github_cloud = mount_cloud(
        document,
        "github-cloud-mount",
        "github.com / api.github.com",
        Terminal::Source,
    );
    let web_clients_cloud = mount_cloud(
        document,
        "web-clients-cloud-mount",
        "web clients",
        Terminal::Sink,
    );
    let durable_cloud = mount_cloud(
        document,
        "durable-cloud-mount",
        "durable substrate",
        Terminal::Sink,
    );
    let sweep_phase_overlay = document
        .get_element_by_id("sweep-phase-overlay-mount")
        .and_then(|container| SweepPhaseBadge::mount(&container));

    AppState {
        sim,
        queue_binding,
        queue_component,
        in_flight_component,
        in_flight_readout: ReadoutStock::new(0.0),
        batch_component,
        batch_readout: ReadoutStock::new(0.0),
        batch_drained_component,
        projection_component,
        projection_readout: ReadoutStock::new(0.0),
        generation_component,
        generation_readout: ReadoutStock::new(0.0),
        served_pages_component,
        served_pages_readout: ReadoutStock::new(0.0),
        events_written_component,
        events_written_readout: ReadoutStock::new(0.0),
        compression_component,
        github_cloud,
        web_clients_cloud,
        durable_cloud,
        sweep_phase_overlay,
        rates,
        tick_count: 0,
        warm_start_requested: false,
        backend_toggle_requested: false,
        last_worker_executions: 0,
        inventory_epoch: 0,
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

    let state = mount_app_state(&document, rates);
    APP.with(|cell| *cell.borrow_mut() = Some(state));

    wire_warm_start_button(&document);
    wire_backend_toggle_button(&document);

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

/// Adapts the app-specific `WorkQueue` binding + running [`Sim`] onto
/// the generic [`StockTemplate`]'s per-field `update_*` calls — the
/// composition [`crate::components::QueueStockComponent`] used to do
/// inline before the stock template generalized (CHE-0094,
/// adr-fmt-odlad SM-2). Lives here, not in `components.rs`, so the
/// template itself stays free of any `Sim`/`QueueStockBinding`
/// knowledge (kind 1 is reusable for stocks with no such binding).
fn update_work_queue_stock(component: &StockTemplate, binding: &QueueStockBinding, sim: &Sim) {
    let ticks_elapsed = component.tick();
    component.update_level(
        binding.level_history(),
        &layout::format_bounded_level(sim.queue_depth(), sim.queue_capacity()),
    );
    component.update_flows(binding.inflow(), Some(binding.outflow()));
    let colors: Vec<&str> = sim
        .queue_jobs()
        .iter()
        .map(|job| crate::components::job_dot_color(job.source))
        .collect();
    component.update_dots(&colors);
    component.update_utilization(binding.utilization().value());
    component.update_residence(binding.mean_residence_ticks(ticks_elapsed));
    component.update_polarity(binding.backpressure_polarity());
}

/// Advances a non-`WorkQueue` Tier-1 stock mount from a bare level
/// readout via [`ReadoutStock::advance`] — the shared adapter every
/// migrated stock in this module (`in_flight`, `BatchTracker`,
/// `EvidenceProjection`, and the three monotonic accumulators) uses in
/// place of [`update_work_queue_stock`]'s richer `QueueStockBinding`
/// wiring, which only `WorkQueue` has (accepted/dequeued event
/// counts).
fn update_readout_stock(
    component: &StockTemplate,
    readout: &mut ReadoutStock,
    new_level: f64,
    level_display: &str,
) {
    let (inflow, outflow) = readout.advance(new_level);
    component.update_level(readout.level_history(), level_display);
    component.update_flows(inflow, Some(outflow));
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

        if state.backend_toggle_requested {
            state.backend_toggle_requested = false;
            let next = match state.sim.durable_backend() {
                PardosaBackend::Pgno => PardosaBackend::Nats,
                PardosaBackend::Nats => PardosaBackend::Pgno,
            };
            state.sim.set_durable_backend(next);
        }

        if state.tick_count.is_multiple_of(7) {
            let _ = state.sim.connect_client();
        }
        if state.tick_count.is_multiple_of(11) && state.sim.ws_permits_in_use() > 0 {
            state.sim.disconnect_client();
        }

        let rates = state.rates.get_untracked();
        let sweep_due = state
            .tick_count
            .is_multiple_of(u64::from(rates.batch_per_tick.max(1)));
        let external_arrival = state
            .tick_count
            .is_multiple_of(u64::from(rates.external_per_tick.max(1)));

        let mut inventory = None;
        if sweep_due {
            let repos = inventory_repos(state.inventory_epoch);
            state.inventory_epoch += 1;
            inventory = Some(state.sim.run_inventory_sweep(&repos, false));
        }

        let events = state.sim.step(false, external_arrival);
        state.tick_count += 1;
        state.queue_binding.advance(&events, &state.sim);
        if let Some(component) = &state.queue_component {
            update_work_queue_stock(component, &state.queue_binding, &state.sim);
        }
        update_sd_components(state);

        render_gauges(&document, &state.sim);
        render_events(&document, &events.arrivals, &events.completions);
        if let Some(outcome) = inventory {
            render_inventory_sweep(&document, outcome);
        }
        render_github_edges(
            &document,
            sweep_due,
            &events.arrivals,
            &mut state.last_worker_executions,
            state.sim.worker_executions(),
        );
        for (update, delivered) in events.page_updates.iter().zip(events.ws_deliveries.iter()) {
            render_read_pulse(&document, *update, true);
            if *delivered > 0 {
                render_ws_fanout(&document);
            }
        }
    });
}

/// Re-renders every migrated SD component mount (Tier-1 stocks, the
/// `barrier_drained`/compression-ratio converters, the boundary
/// clouds, and the non-SD `SweepPhase` overlay) from the current
/// [`Sim`] state — split out of [`tick`] to stay under clippy's
/// function-length bar.
#[expect(
    clippy::cast_precision_loss,
    reason = "sim counters (in_flight, batch_remaining, repos captured, generation, served pages, events written) are bounded well under 2^52 for any realistic sim run"
)]
fn update_sd_components(state: &mut AppState) {
    let sim = &state.sim;

    if let Some(component) = &state.in_flight_component {
        update_readout_stock(
            component,
            &mut state.in_flight_readout,
            sim.in_flight() as f64,
            &layout::format_bounded_level(sim.in_flight(), sim.worker_count()),
        );
        let utilization = if sim.worker_count() > 0 {
            sim.in_flight() as f64 / sim.worker_count() as f64
        } else {
            0.0
        };
        component.update_utilization(utilization);
    }

    if let Some(component) = &state.batch_component {
        update_readout_stock(
            component,
            &mut state.batch_readout,
            sim.batch_remaining() as f64,
            &sim.batch_remaining().to_string(),
        );
    }
    if let Some(component) = &state.batch_drained_component {
        let drained = if sim.batch_remaining() == 0 {
            "yes"
        } else {
            "no"
        };
        component.update(drained);
    }

    if let Some(component) = &state.projection_component {
        update_readout_stock(
            component,
            &mut state.projection_readout,
            sim.repositories_captured() as f64,
            &sim.repositories_captured().to_string(),
        );
    }

    if let Some(component) = &state.generation_component {
        update_readout_stock(
            component,
            &mut state.generation_readout,
            sim.arcswap_generation() as f64,
            &sim.arcswap_generation().to_string(),
        );
    }
    if let Some(component) = &state.served_pages_component {
        update_readout_stock(
            component,
            &mut state.served_pages_readout,
            sim.served_pages() as f64,
            &sim.served_pages().to_string(),
        );
    }
    if let Some(component) = &state.events_written_component {
        update_readout_stock(
            component,
            &mut state.events_written_readout,
            sim.events_written() as f64,
            &sim.events_written().to_string(),
        );
    }

    if let Some(component) = &state.compression_component {
        component.update(&compression_ratio_display(sim));
    }

    if let Some(cloud) = &state.github_cloud {
        cloud.update(u64::from(sim.github_calls_used()));
    }
    if let Some(cloud) = &state.web_clients_cloud {
        cloud.update(sim.served_pages() as u64);
    }
    if let Some(cloud) = &state.durable_cloud {
        cloud.update(sim.events_written() as u64);
    }

    if let Some(overlay) = &state.sweep_phase_overlay {
        overlay.update(sweep_phase_label(sim.sweep_phase()));
    }
}

/// A small rotating inventory of `(baseline_updated_at,
/// current_updated_at)` pairs mirroring one `build_inventory_from_api`
/// listing (inventory.rs:50). Roughly half the repos keep an unchanged
/// `updated_at` (reused from the projection, no job) and half advance
/// it (spawning a [`JobSource::ScheduledBatch`] job) so the
/// `should_reuse` gate (baseline.rs:65) visibly splits every sweep.
fn inventory_repos(epoch: u64) -> Vec<(UpdatedAt, UpdatedAt)> {
    const REPO_COUNT: u64 = 8;
    (0..REPO_COUNT)
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

/// Pulses [`PATH_GITHUB_INVENTORY`] (the `build_inventory_from_api`
/// listing arriving at the sweep) and updates the gauges reporting the
/// `should_reuse` split: repos inventoried, repos reused unchanged (no
/// job), and jobs spawned for changed repos.
fn render_inventory_sweep(document: &Document, outcome: InventoryOutcome) {
    set_text(
        document,
        "inventory-inventoried",
        &outcome.inventoried.to_string(),
    );
    set_text(
        document,
        "inventory-reused",
        &outcome.reused_unchanged.to_string(),
    );
    set_text(
        document,
        "inventory-spawned",
        &outcome.jobs_spawned.to_string(),
    );
    let Some(packet_layer) = document.get_element_by_id("packet-layer") else {
        return;
    };
    spawn_transit_packet(
        document,
        &packet_layer,
        &PacketSpec {
            class: "packet packet-inventory",
            color: "#e2e8f0",
            path_d: PATH_GITHUB_INVENTORY,
            start: "0%",
            end: "100%",
            duration_ms: GITHUB_INVENTORY_DURATION_MS,
        },
    );
}

/// Pulses [`PATH_GITHUB_PUSH`] once per webhook arrival (`github.com`
/// pushing the delivery `webhook_handler` receives, independent of the
/// enqueue outcome), [`PATH_GITHUB_INVENTORY`] once per scheduled sweep
/// (`github.com` serving the `build_inventory_from_api` listing to the
/// sweep), and [`PATH_GITHUB_PULL`] once per new worker dispatch
/// (`worker_loop`/`LiveEvaluator::evaluate` pulling `repo_details` +
/// the six concurrent collector calls, gated by
/// `RateLimitState`/`BudgetGate`).
fn render_github_edges(
    document: &Document,
    inventory_listed: bool,
    arrivals: &[(JobSource, EnqueueResult)],
    last_worker_executions: &mut u64,
    current_worker_executions: u64,
) {
    let Some(packet_layer) = document.get_element_by_id("packet-layer") else {
        return;
    };
    if inventory_listed {
        spawn_transit_packet(
            document,
            &packet_layer,
            &PacketSpec {
                class: "packet packet-github-inventory",
                color: "#cbd5e1",
                path_d: PATH_GITHUB_INVENTORY,
                start: "0%",
                end: "100%",
                duration_ms: GITHUB_INVENTORY_DURATION_MS,
            },
        );
    }
    if arrivals
        .iter()
        .any(|(source, _)| matches!(source, JobSource::External { .. }))
    {
        spawn_transit_packet(
            document,
            &packet_layer,
            &PacketSpec {
                class: "packet packet-github-push",
                color: "#e2e8f0",
                path_d: PATH_GITHUB_PUSH,
                start: "0%",
                end: "100%",
                duration_ms: GITHUB_PUSH_DURATION_MS,
            },
        );
    }
    if current_worker_executions > *last_worker_executions {
        spawn_transit_packet(
            document,
            &packet_layer,
            &PacketSpec {
                class: "packet packet-github-pull",
                color: "#6366f1",
                path_d: PATH_GITHUB_PULL,
                start: "0%",
                end: "100%",
                duration_ms: GITHUB_PULL_DURATION_MS,
            },
        );
    }
    *last_worker_executions = current_worker_executions;
}

/// Pulses [`PATH_CLIENTS_WS`] once per `PageUpdateEvent` delivered to
/// at least one connected sim client (`ClientPool::broadcast`).
fn render_ws_fanout(document: &Document) {
    let Some(packet_layer) = document.get_element_by_id("packet-layer") else {
        return;
    };
    spawn_transit_packet(
        document,
        &packet_layer,
        &PacketSpec {
            class: "packet packet-ws-fanout",
            color: "#f59e0b",
            path_d: PATH_CLIENTS_WS,
            start: "0%",
            end: "100%",
            duration_ms: CLIENT_WS_DURATION_MS,
        },
    );
}

/// Residual (not-yet-migrated) gauges only — Tier-1 stocks, converters,
/// and clouds render through their mounted SD components
/// ([`update_sd_components`]) instead. `memo-hits`/`memo-rebuilds` are
/// Tier-3 incidental (adr-fmt-vrycy); `queue-full`/`deduplicated`/
/// `failure` are `WorkQueue` outcome counters with no template mapped
/// this increment; `worker-executions` labels the `worker_loop`
/// process node, not a Stock.
fn render_gauges(document: &Document, sim: &Sim) {
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
    set_text(document, "memo-hits", &sim.memo_hits().to_string());
    set_text(document, "memo-rebuilds", &sim.memo_rebuilds().to_string());
    set_text(
        document,
        "worker-executions",
        &sim.worker_executions().to_string(),
    );
    render_external_gauges(document, sim);
}

/// Component A/B gauges (durable-store backend, `ClientPool`,
/// `BudgetGate`) — split out of [`render_gauges`] to stay under
/// clippy's function-length bar.
fn render_external_gauges(document: &Document, sim: &Sim) {
    set_text(
        document,
        "backend-label",
        backend_label(sim.durable_backend()),
    );
    set_text(
        document,
        "native-events-written",
        &sim.native_events_written().to_string(),
    );
    set_text(
        document,
        "jetstream-sequence",
        &sim.jetstream_sequence().to_string(),
    );
    toggle_backend_nodes(document, sim.durable_backend());
    set_text(
        document,
        "ws-permits",
        &format!("{}/{}", sim.ws_permits_in_use(), sim.ws_max_connections()),
    );
    set_text(
        document,
        "ws-permits-legend",
        &format!("{}/{}", sim.ws_permits_in_use(), sim.ws_max_connections()),
    );
    set_text(
        document,
        "github-budget",
        &format!("{}/{}", sim.github_calls_used(), sim.github_budget()),
    );
}

fn backend_label(backend: PardosaBackend) -> &'static str {
    match backend {
        PardosaBackend::Pgno => "Pgno",
        PardosaBackend::Nats => "Nats",
    }
}

/// Dims whichever durable-store backend node is inactive so the single
/// `NativeStore::record` facade (store/mod.rs:132) visibly routes to
/// exactly ONE backend selected by `PardosaBackend` (config/runtime.rs:
/// 42-43) — the local `.pgno` store when `Pgno`, `NATS` `JetStream`
/// when `Nats` — never a simultaneous fan-out to both.
fn toggle_backend_nodes(document: &Document, backend: PardosaBackend) {
    let (active_id, inactive_id) = match backend {
        PardosaBackend::Pgno => ("backend-pgno", "backend-nats"),
        PardosaBackend::Nats => ("backend-nats", "backend-pgno"),
    };
    if let Some(active) = document.get_element_by_id(active_id) {
        active.class_list().remove_1("backend-dimmed").ok();
    }
    if let Some(inactive) = document.get_element_by_id(inactive_id) {
        inactive.class_list().add_1("backend-dimmed").ok();
    }
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
/// never touches the gate. The event's `generation` is not
/// re-rendered here — [`update_sd_components`] already synced the
/// `generation` stock mount from the current [`Sim`] state earlier
/// this tick.
fn render_read_pulse(document: &Document, _update: PageUpdateEvent, gated: bool) {
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

/// Wires the `--pardosa-backend`-mirroring toggle: an operator
/// selection between [`PardosaBackend::Pgno`] (default) and
/// [`PardosaBackend::Nats`] (alternate), never an automatic sim
/// behaviour.
fn wire_backend_toggle_button(document: &Document) {
    let Some(element) = document.get_element_by_id("backend-toggle-btn") else {
        return;
    };
    let closure = wasm_bindgen::closure::Closure::<dyn FnMut(web_sys::Event)>::new(move |_event| {
        APP.with(|cell| {
            if let Some(state) = cell.borrow_mut().as_mut() {
                state.backend_toggle_requested = true;
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
/// absolutely-positioned HTML node boxes, sharing the same 1280x800
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
    <svg class='graph-svg' viewBox='0 0 1280 800' preserveAspectRatio='xMidYMid meet'>
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
      <path class='edge edge-github-push' d='{PATH_GITHUB_PUSH}' marker-end='url(#arrow)' />
      <path class='edge edge-github-inventory' d='{PATH_GITHUB_INVENTORY}' marker-end='url(#arrow)' />
      <path class='edge edge-github-pull' d='{PATH_GITHUB_PULL}' marker-end='url(#arrow)' />
      <path class='edge edge-backend' d='{PATH_BACKEND_PGNO}' marker-end='url(#arrow)' />
      <path class='edge edge-backend' d='{PATH_BACKEND_NATS}' marker-end='url(#arrow)' />
      <path class='edge edge-clients-http' d='{PATH_CLIENTS_HTTP}' marker-end='url(#arrow)' />
      <path class='edge edge-clients-ws' d='{PATH_CLIENTS_WS}' marker-end='url(#arrow)' />
    </svg>
    <div id='packet-layer' class='packet-layer'></div>
{}
  </div>

  <div class='row legend'>
    <div class='gauge'>QueueFull <span id='queue-full-count'>0</span></div>
    <div class='gauge'>Deduplicated <span id='deduplicated-count'>0</span></div>
    <div class='gauge'>failures <span id='failure-count'>0</span></div>
  </div>
  <div class='row legend'>
    <div class='gauge'>memo hits <span id='memo-hits'>0</span></div>
    <div class='gauge'>memo rebuilds <span id='memo-rebuilds'>0</span></div>
  </div>
  <div class='row legend'>
    <div class='gauge'>ws permits/cap <span id='ws-permits-legend'>0/200</span></div>
    <div class='gauge'>GitHub calls/epoch <span id='github-budget'>0/0</span></div>
  </div>
</section>
",
        graph_nodes_markup(),
    )
}

/// Node boxes only (triggers, write spine, gate, read chain, serve),
/// split out of [`graph_markup`] purely to stay under clippy's
/// function-length bar. Standardized SD components (Tier-1 stocks,
/// the `barrier_drained`/compression-ratio converters, boundary
/// clouds, and the non-SD `SweepPhase` overlay) are bare mount `div`s
/// filled in by [`crate::components`]/[`crate::overlay`] at
/// [`start`](self::start) time; the surrounding wrapper prose (repo
/// listing counts, backend-selection controls) stays ad-hoc where it
/// documents a Tier-2/3 process rather than a Tier-1 SD element.
fn graph_nodes_markup() -> &'static str {
    r"
    <div class='node node-external node-github' style='left:555px;top:55px'>
      <span class='stage-note'>push &rarr; webhook_handler</span>
      <span class='stage-note'>inventory listing &rarr; sweep: build_inventory_from_api
        (GET /orgs/&lbrace;org&rbrace;/repos?type=all)</span>
      <span class='stage-note'>pull &larr; worker: GitHubClient::repo_details</span>
      <span class='stage-note'>6&times; security_policy/ghas_scanning/dependabot/
        branch_protection/codeowners::evaluate + last_commit::fetch_last_commit</span>
      <span class='stage-note'>RateLimitState + BudgetGate &rarr; ApiOutcome &rarr; RepositoryEvidence</span>
      <div id='github-cloud-mount' class='node-sd-mount'></div>
    </div>

    <div class='node node-trigger node-scheduled' style='left:110px;top:120px'>
      spawn_collection_loop / SweepSaga
      <div id='sweep-phase-overlay-mount' class='phase-overlay'></div>
      <span class='stage-note'>inventory listing (InventoryLoad): <span id='inventory-inventoried'>0</span> repos</span>
      <span class='stage-note'>should_reuse: reused <span id='inventory-reused'>0</span> (no job)
        | ScheduledBatch spawned <span id='inventory-spawned'>0</span> (updated_at changed)</span>
    </div>
    <div class='node node-trigger node-webhook' style='left:110px;top:300px'>
      webhook_handler
      <span class='stage-note'>execute_enqueue JobSource::External&lbrace;id,kind&rbrace;</span>
    </div>
    <div class='node node-trigger node-warmstart' style='left:110px;top:470px'>
      warm_start_from_baseline
      <span class='stage-note'>render-only bypass (NO enqueue)</span>
      <button id='warm-start-btn'>fire warm start</button>
    </div>

    <div id='queue-stock-mount' class='node node-store node-queue node-sd-stock' style='left:350px;top:180px'></div>
    <div id='in-flight-stock-mount' class='node node-store node-sd-stock' style='left:555px;top:180px'></div>
    <div class='node node-work node-worker' style='left:555px;top:275px'>
      worker_loop / LiveEvaluator::evaluate
      <span class='stage-note'>executions <span id='worker-executions'>0</span></span>
    </div>
    <div class='node node-store node-eventstream' style='left:790px;top:180px'>
      record_repo &rarr; NativeStore::record
      <span class='stage-note'>active PardosaBackend::<span id='backend-label'>Pgno</span></span>
      <button id='backend-toggle-btn'>toggle Pgno/Nats</button>
    </div>
    <div id='events-written-stock-mount' class='node node-store node-sd-stock' style='left:790px;top:275px'></div>
    <div id='backend-pgno' class='node node-store node-backend-pgno' style='left:690px;top:340px'>
      local .pgno file store
      <span class='stage-note'>events.pgno / org-events.pgno</span>
      <span class='stage-note'>appended <span id='native-events-written'>0</span></span>
    </div>
    <div id='backend-nats' class='node node-store node-backend-nats' style='left:890px;top:340px'>
      NATS JetStream
      <span class='stage-note'>JetStreamHandle::append &rarr; PubAck seq</span>
      <span class='stage-note'>seq <span id='jetstream-sequence'>0</span></span>
    </div>
    <div id='projection-stock-mount' class='node node-store node-sd-stock' style='left:1010px;top:180px'></div>
    <div id='durable-cloud-mount' class='node node-store node-sd-mount' style='left:1010px;top:275px'></div>

    <div id='gate-glyph' class='gate-glyph' style='left:955px;top:230px'>
      &#9670;
    </div>
    <div id='batch-stock-mount' class='node node-store node-sd-stock' style='left:955px;top:300px'></div>
    <div id='batch-drained-mount' class='node node-work node-sd-mount' style='left:955px;top:390px'></div>

    <div class='node node-work node-finalize' style='left:900px;top:440px'>
      finalize_and_publish
      <span class='stage-note'>per RUN</span>
    </div>
    <div class='node node-work node-buildcache' style='left:1050px;top:440px'>
      build_cached_pages
      <span class='stage-note'>memo</span>
    </div>
    <div id='compression-converter-mount' class='node node-work node-sd-mount' style='left:1050px;top:520px'></div>
    <div class='node node-work node-commit' style='left:1200px;top:440px'>
      commit_cached_pages
    </div>
    <div id='generation-stock-mount' class='node node-store node-sd-stock' style='left:1200px;top:530px'></div>

    <div class='node node-serve node-served' style='left:1200px;top:640px'>
      cache_fallback &rarr; served pages
    </div>
    <div id='served-pages-stock-mount' class='node node-store node-sd-stock' style='left:1050px;top:640px'></div>

    <div class='node node-clients node-webclients' style='left:1200px;top:730px'>
      ws_session clients (anonymous)
      <span class='stage-note'>OwnedSemaphorePermit + broadcast::Receiver&lt;PageUpdateEvent&gt;</span>
      <span class='stage-note'>permits/cap <span id='ws-permits'>0/200</span> (sim quantity)</span>
      <div id='web-clients-cloud-mount' class='node-sd-mount'></div>
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
