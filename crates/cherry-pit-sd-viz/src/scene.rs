//! Host-pure, declarative Scene model (adr-fmt-sra3p, `svs-02`):
//! composes [`crate::sd::Model`]'s already-declared element set with a
//! placement overlay into the single data structure the ENTIRE visual
//! derives from. No wasm, no DOM/SVG emission, no `web-sys` — those
//! belong to the (future) wasm-gated interpreter that walks this
//! `Scene`, never the reverse (COM-0012 inward-only dependency rule).
//!
//! # Shape (feynman H1, `adr-fmt-ogosr`)
//!
//! [`Model`] stays the single structural source of truth: it already
//! enforces the adr-fmt-qaavg connection grammar at compile time (the
//! sealed [`crate::sd::FlowEndpoint`]/[`crate::sd::ConnectorTail`]/
//! [`crate::sd::ConnectorHead`] traits) and at `build()` time (the
//! cloud-to-cloud degenerate-flow check). [`Scene`] adds ONLY a
//! placement + presentation overlay keyed on the model's
//! [`StockId`]/[`CloudId`]/[`ConverterId`] handles: a [`GridSlot`] per
//! node plus a label. Belts are never re-declared by hand — every
//! [`Belt`] is derived from a [`FlowView`] the model already exposes
//! (`svs-01`'s `Model::flows()`), so a belt connecting an
//! undeclared/dangling endpoint is impossible by construction: the
//! only way to get a [`Belt`] is to route one of the model's own
//! [`FlowView`]s through the placement overlay's slot lookup.
//!
//! # Belt motion (feynman Orientation 2)
//!
//! Two composable motion classes, matching the two kinds of belt
//! traffic the redesign distinguishes:
//!
//! - **Continuous flow-rate belts** (the SD spine: arrivals, dequeue,
//!   serve) — [`belt_item_count`] + [`belt_item_phase`], a stateless
//!   periodic phase function (shapez2 continuous-conveyor even
//!   spacing). No per-item state; every tick recomputes every item's
//!   position fresh from `(k, t, speed, length, spacing)`.
//! - **Discrete event pulses** (a specific failed job, a warm-start) —
//!   [`BeltItemLog`], a bounded (default cap [`MAX_BELT_ITEMS`]),
//!   prunable item list carrying per-item identity
//!   ([`crate::sim::JobSource`]), lifting the old `MAX_LANE_PACKETS`
//!   DOM-child-count discipline into a host-pure `Vec`.

use std::collections::{HashMap, VecDeque};

use crate::layout::{self, GridParams, Side};
use crate::sd::{
    CloudId, ConverterId, Flow, FlowId, FlowTerminal, Model, SdConnectionError, StockId,
};
use crate::sim::JobSource;

/// A zero-indexed `(row, col)` position in the presentation grid —
/// the same coordinate space [`crate::layout::grid_slot_origin`] and
/// [`crate::layout::slot_anchor`] consume. Distinct from a raw
/// `(usize, usize)` tuple so a [`Placement`] entry reads as "this
/// node's slot", not an anonymous pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GridSlot {
    pub row: usize,
    pub col: usize,
}

impl GridSlot {
    #[must_use]
    pub fn new(row: usize, col: usize) -> Self {
        Self { row, col }
    }
}

/// A cloud terminal's presentation role, supplied by the placement
/// overlay (not read back from [`Model`] — clouds carry no public
/// role accessor, and this iteration keeps `sd.rs` untouched). The
/// overlay author already knows a cloud's role when placing it (the
/// same knowledge [`crate::binding::tier1_model`]'s doc comment
/// records), so this is presentation metadata layered on top of the
/// model, not a duplicate of [`crate::sd::Terminal`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloudRole {
    Source,
    Sink,
}

/// One placed node: a model handle, discriminated by kind, plus its
/// [`GridSlot`] and a display label. Deliberately a discriminated
/// union over existing `sd::*Id` handles rather than a new type per
/// kind (COM-0002:R5 wrapper-mirror red flag) — [`NodeGeometry`]
/// carries no fields [`Model`] doesn't already own; it only adds the
/// kind discriminant placement needs to route each node's box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeGeometry {
    Stock(StockId, layout::StockKind),
    Cloud(CloudId, CloudRole),
    Converter(ConverterId),
}

/// A [`NodeGeometry`] placed at a [`GridSlot`] under a display label —
/// everything an interpreter needs to mount one node's box, still
/// zero DOM/SVG.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlacedNode {
    pub geometry: NodeGeometry,
    pub slot: GridSlot,
    pub label: &'static str,
}

/// One routed belt: a model [`FlowId`] plus the anchor points and SVG
/// path [`crate::layout::slot_anchor`]/[`crate::layout::bezier_edge_path`]
/// derive from its two endpoints' placed slots. `length` is the
/// [`crate::layout::cubic_arc_length`] estimate belt-item motion
/// samples against.
#[derive(Debug, Clone, PartialEq)]
pub struct Belt {
    pub id: FlowId,
    pub from: (f64, f64),
    pub control: ((f64, f64), (f64, f64)),
    pub to: (f64, f64),
    pub path: String,
    pub length: f64,
    pub kind: Flow,
}

impl Belt {
    /// The `(x, y)` point at fraction `t` (`0.0..=1.0`, clamped) along
    /// this belt's routed curve.
    #[must_use]
    pub fn point_at(&self, t: f64) -> (f64, f64) {
        layout::cubic_point_at(self.from, self.control, self.to, t)
    }
}

/// Errors deriving a [`Scene`] from a [`Model`] and its placement
/// overlay. `#[non_exhaustive]` per CHE-0094:R13/CHE-0021 — new
/// placement-completeness rules may add variants without a breaking
/// change.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScenePlacementError {
    /// A model-declared [`StockId`]/[`CloudId`]/[`ConverterId`] has no
    /// entry in the placement overlay — the abort condition "a Tier-1
    /// element has no model handle to key placement on" inverted: here
    /// the handle exists but the overlay is missing a slot for it.
    MissingSlot,
}

/// The placement + presentation overlay: one [`GridSlot`] and label
/// per model handle, plus the shared [`GridParams`] every anchor and
/// dimension computation routes through. Built once, then consumed by
/// [`Scene::assemble`] to derive placed nodes and routed belts.
#[derive(Debug, Clone, Default)]
pub struct Placement {
    grid: Option<GridParams>,
    stocks: HashMap<StockId, (GridSlot, layout::StockKind, &'static str)>,
    clouds: HashMap<CloudId, (GridSlot, CloudRole, &'static str)>,
    converters: HashMap<ConverterId, (GridSlot, &'static str)>,
}

impl Placement {
    #[must_use]
    pub fn new(grid: GridParams) -> Self {
        Self {
            grid: Some(grid),
            ..Self::default()
        }
    }

    #[must_use]
    pub fn with_stock(
        mut self,
        id: StockId,
        slot: GridSlot,
        kind: layout::StockKind,
        label: &'static str,
    ) -> Self {
        self.stocks.insert(id, (slot, kind, label));
        self
    }

    #[must_use]
    pub fn with_cloud(
        mut self,
        id: CloudId,
        slot: GridSlot,
        role: CloudRole,
        label: &'static str,
    ) -> Self {
        self.clouds.insert(id, (slot, role, label));
        self
    }

    #[must_use]
    pub fn with_converter(mut self, id: ConverterId, slot: GridSlot, label: &'static str) -> Self {
        self.converters.insert(id, (slot, label));
        self
    }

    fn slot_of_stock(&self, id: StockId) -> Option<GridSlot> {
        self.stocks.get(&id).map(|(slot, ..)| *slot)
    }

    fn slot_of_cloud(&self, id: CloudId) -> Option<GridSlot> {
        self.clouds.get(&id).map(|(slot, ..)| *slot)
    }

    fn slot_of_terminal(&self, terminal: FlowTerminal) -> Option<GridSlot> {
        match terminal {
            FlowTerminal::Stock(id) => self.slot_of_stock(id),
            FlowTerminal::Cloud(id) => self.slot_of_cloud(id),
        }
    }
}

/// The Scene proper: a [`Model`] plus its [`Placement`] overlay,
/// assembled into placed nodes and routed belts. The whole visual is
/// `nodes()` + `belts()` — a data definition, not imperative
/// construction; moving a node is editing one [`Placement`] entry,
/// adding a node is one `Model::add_*` call plus one placement entry.
pub struct Scene {
    model: Model,
    grid: GridParams,
    nodes: Vec<PlacedNode>,
    belts: Vec<Belt>,
}

impl Scene {
    /// Derives a [`Scene`] from `model` and `placement`: every
    /// registered stock/cloud/converter must have a placement entry
    /// ([`ScenePlacementError::MissingSlot`] otherwise), and every
    /// model [`FlowView`](crate::sd::FlowView) is routed into a
    /// [`Belt`] via the two endpoints' placed slots — belts are never
    /// hand-declared, so a dangling belt (an edge naming an
    /// undeclared endpoint) cannot occur.
    ///
    /// # Errors
    ///
    /// Returns [`ScenePlacementError::MissingSlot`] when any
    /// model-registered handle lacks a placement entry.
    pub fn assemble(model: Model, placement: &Placement) -> Result<Self, ScenePlacementError> {
        let grid = placement.grid.ok_or(ScenePlacementError::MissingSlot)?;

        let mut nodes = Vec::new();
        for id in model.stock_ids() {
            let (slot, kind, label) = placement
                .stocks
                .get(&id)
                .copied()
                .ok_or(ScenePlacementError::MissingSlot)?;
            nodes.push(PlacedNode {
                geometry: NodeGeometry::Stock(id, kind),
                slot,
                label,
            });
        }
        for id in model.cloud_ids() {
            let (slot, role, label) = placement
                .clouds
                .get(&id)
                .copied()
                .ok_or(ScenePlacementError::MissingSlot)?;
            nodes.push(PlacedNode {
                geometry: NodeGeometry::Cloud(id, role),
                slot,
                label,
            });
        }
        for id in model.converter_ids() {
            let (slot, label) = placement
                .converters
                .get(&id)
                .copied()
                .ok_or(ScenePlacementError::MissingSlot)?;
            nodes.push(PlacedNode {
                geometry: NodeGeometry::Converter(id),
                slot,
                label,
            });
        }

        let mut belts = Vec::with_capacity(model.flow_count());
        for view in model.flows() {
            let tail_slot = placement
                .slot_of_terminal(view.tail)
                .ok_or(ScenePlacementError::MissingSlot)?;
            let head_slot = placement
                .slot_of_terminal(view.head)
                .ok_or(ScenePlacementError::MissingSlot)?;
            let (tail_side, head_side) = anchor_side_pair(tail_slot, head_slot);
            let from = layout::slot_anchor(tail_slot.row, tail_slot.col, tail_side, grid);
            let to = layout::slot_anchor(head_slot.row, head_slot.col, head_side, grid);
            let control = layout::bezier_control_points(from, to);
            let path = layout::bezier_edge_path(from, to);
            let length = layout::cubic_arc_length(from, control, to, 32);
            belts.push(Belt {
                id: view.id,
                from,
                control,
                to,
                path,
                length,
                kind: view.kind,
            });
        }

        Ok(Self {
            model,
            grid,
            nodes,
            belts,
        })
    }

    #[must_use]
    pub fn model(&self) -> &Model {
        &self.model
    }

    #[must_use]
    pub fn nodes(&self) -> &[PlacedNode] {
        &self.nodes
    }

    #[must_use]
    pub fn belts(&self) -> &[Belt] {
        &self.belts
    }

    /// The shared [`GridParams`] every node box and belt anchor in
    /// this scene was derived from — the renderer's only source for a
    /// node box's px width/height (it must never hard-code its own).
    #[must_use]
    pub fn grid(&self) -> GridParams {
        self.grid
    }

    /// The top-left `(x, y)` px origin of `node`'s box — the
    /// renderer's only source for a node's position; it must never
    /// compute a position itself.
    #[must_use]
    pub fn node_origin(&self, node: &PlacedNode) -> (f64, f64) {
        layout::grid_slot_origin(node.slot.row, node.slot.col, self.grid)
    }

    /// The `(width, height)` px viewBox this scene's nodes fit inside,
    /// derived from the highest occupied row/col among the placed
    /// nodes — the renderer's only source for its SVG `viewBox`; it
    /// must never compute its own bound.
    #[must_use]
    pub fn viewbox_dimensions(&self) -> (f64, f64) {
        let max_row = self
            .nodes
            .iter()
            .map(|node| node.slot.row)
            .max()
            .unwrap_or(0);
        let max_col = self
            .nodes
            .iter()
            .map(|node| node.slot.col)
            .max()
            .unwrap_or(0);
        layout::grid_dimensions(max_row + 1, max_col + 1, self.grid)
    }
}

/// Picks which [`Side`] of the tail's box and which [`Side`] of the
/// head's box a belt should anchor to, from the two slots' relative
/// grid position: a lower row anchors `Bottom`->`Top` (the common
/// vertical source-to-client flow), a higher row `Top`->`Bottom`, and
/// same-row slots anchor by column via `Right`->`Left` or
/// `Left`->`Right`.
fn anchor_side_pair(tail: GridSlot, head: GridSlot) -> (Side, Side) {
    match tail.row.cmp(&head.row) {
        std::cmp::Ordering::Less => (Side::Bottom, Side::Top),
        std::cmp::Ordering::Greater => (Side::Top, Side::Bottom),
        std::cmp::Ordering::Equal => {
            if tail.col <= head.col {
                (Side::Right, Side::Left)
            } else {
                (Side::Left, Side::Right)
            }
        }
    }
}

/// Builds the gh-report [`Scene`]: [`crate::binding::tier1_model`]'s
/// 7 stocks / 5 clouds / 3 converters / 11 flows, placed on a 5-row
/// grid honoring the source-top-left / client-bottom-right causal
/// order (adr-fmt-vrycy): row 0 the two source clouds, row 1 the
/// queue-processing spine (`work_queue`, `in_flight`), row 2 the
/// join-barrier stock plus all three converters, row 3 the three
/// monotonic readout accumulators, row 4 the three sink clouds. Every
/// row holds at most 5 nodes (row-plan cap 6 per
/// [`crate::layout`]'s existing rule).
///
/// # Errors
///
/// Propagates [`SdConnectionError`] from
/// [`crate::binding::tier1_model`] (expected `Ok`; the `Err` path
/// exists so callers can assert legality rather than assume it) and
/// [`ScenePlacementError`] should the placement overlay below ever
/// drift out of sync with the model's registration order (guarded by
/// this module's own tests).
pub fn gh_report_scene() -> Result<Scene, GhReportSceneError> {
    let model = crate::binding::tier1_model().map_err(GhReportSceneError::Connection)?;
    let placement = gh_report_placement(&model)?;
    Scene::assemble(model, &placement).map_err(GhReportSceneError::Placement)
}

/// The gh-report Tier-1 spine's handles, extracted from `model`'s
/// insertion-order enumeration in [`crate::binding::tier1_model`]'s
/// documented registration order.
struct Tier1Handles {
    work_queue: StockId,
    in_flight: StockId,
    batch_remaining: StockId,
    evidence_projection: StockId,
    generation: StockId,
    served_pages: StockId,
    events_written: StockId,
    timer_source: CloudId,
    github_source: CloudId,
    github_sink: CloudId,
    web_clients_sink: CloudId,
    durable_sink: CloudId,
    utilization: ConverterId,
    barrier_drained: ConverterId,
    read_side: ConverterId,
}

fn tier1_handles(model: &Model) -> Result<Tier1Handles, GhReportSceneError> {
    let missing = || GhReportSceneError::MissingHandle;
    let mut stock_ids = model.stock_ids();
    let mut cloud_ids = model.cloud_ids();
    let mut converter_ids = model.converter_ids();
    Ok(Tier1Handles {
        work_queue: stock_ids.next().ok_or_else(missing)?,
        in_flight: stock_ids.next().ok_or_else(missing)?,
        batch_remaining: stock_ids.next().ok_or_else(missing)?,
        evidence_projection: stock_ids.next().ok_or_else(missing)?,
        generation: stock_ids.next().ok_or_else(missing)?,
        served_pages: stock_ids.next().ok_or_else(missing)?,
        events_written: stock_ids.next().ok_or_else(missing)?,
        timer_source: cloud_ids.next().ok_or_else(missing)?,
        github_source: cloud_ids.next().ok_or_else(missing)?,
        github_sink: cloud_ids.next().ok_or_else(missing)?,
        web_clients_sink: cloud_ids.next().ok_or_else(missing)?,
        durable_sink: cloud_ids.next().ok_or_else(missing)?,
        utilization: converter_ids.next().ok_or_else(missing)?,
        barrier_drained: converter_ids.next().ok_or_else(missing)?,
        read_side: converter_ids.next().ok_or_else(missing)?,
    })
}

/// Builds the hand-authored [`Placement`] overlay for
/// [`gh_report_scene`], keyed on `model`'s stock/cloud/converter
/// handles in [`crate::binding::tier1_model`]'s documented insertion
/// order. Split out from [`gh_report_scene`] purely to keep each
/// function under the crate's line-count lint ceiling — no behavioural
/// difference from inlining it.
fn gh_report_placement(model: &Model) -> Result<Placement, GhReportSceneError> {
    let grid = GridParams {
        margin: 40.0,
        col_pitch: 205.0,
        row_pitch: 200.0,
        box_width: 180.0,
        box_height: 92.0,
    };
    let h = tier1_handles(model)?;

    Ok(Placement::new(grid)
        .with_cloud(
            h.timer_source,
            GridSlot::new(0, 0),
            CloudRole::Source,
            "Timer",
        )
        .with_cloud(
            h.github_source,
            GridSlot::new(0, 1),
            CloudRole::Source,
            "GitHub",
        )
        .with_stock(
            h.work_queue,
            GridSlot::new(1, 0),
            layout::StockKind::Standard,
            "WorkQueue",
        )
        .with_stock(
            h.in_flight,
            GridSlot::new(1, 1),
            layout::StockKind::Bounded,
            "InFlight",
        )
        .with_stock(
            h.batch_remaining,
            GridSlot::new(2, 0),
            layout::StockKind::Accumulator,
            "BatchRemaining",
        )
        .with_stock(
            h.evidence_projection,
            GridSlot::new(2, 1),
            layout::StockKind::Accumulator,
            "EvidenceProjection",
        )
        .with_converter(h.utilization, GridSlot::new(2, 2), "Utilization")
        .with_converter(h.barrier_drained, GridSlot::new(2, 3), "BarrierDrained")
        .with_converter(h.read_side, GridSlot::new(2, 4), "ReadSide")
        .with_stock(
            h.generation,
            GridSlot::new(3, 0),
            layout::StockKind::Monotonic,
            "Generation",
        )
        .with_stock(
            h.served_pages,
            GridSlot::new(3, 1),
            layout::StockKind::Monotonic,
            "ServedPages",
        )
        .with_stock(
            h.events_written,
            GridSlot::new(3, 2),
            layout::StockKind::Monotonic,
            "EventsWritten",
        )
        .with_cloud(
            h.github_sink,
            GridSlot::new(4, 0),
            CloudRole::Sink,
            "GitHubSink",
        )
        .with_cloud(
            h.web_clients_sink,
            GridSlot::new(4, 1),
            CloudRole::Sink,
            "WebClients",
        )
        .with_cloud(
            h.durable_sink,
            GridSlot::new(4, 2),
            CloudRole::Sink,
            "DurableStore",
        ))
}

/// Errors building the gh-report [`Scene`] via [`gh_report_scene`].
/// `#[non_exhaustive]` per CHE-0094:R13/CHE-0021.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GhReportSceneError {
    /// [`crate::binding::tier1_model`] rejected the wiring.
    Connection(SdConnectionError),
    /// The hand-authored placement overlay assumed a handle that the
    /// model's insertion-order enumeration did not yield — signals
    /// the overlay has drifted out of sync with `tier1_model`'s
    /// registration order.
    MissingHandle,
    /// [`Scene::assemble`] found a registered handle with no
    /// placement entry.
    Placement(ScenePlacementError),
}

/// The default cap on how many discrete [`BeltItem`]s a
/// [`BeltItemLog`] retains, mirroring the old `MAX_LANE_PACKETS = 40`
/// DOM-child-count discipline lifted into a host-pure bounded `Vec`.
pub const MAX_BELT_ITEMS: usize = 40;

/// The number of evenly-spaced items a continuous-flow belt of
/// `length` renders at `spacing` apart: `floor(length / spacing)`.
/// `0` when `length` or `spacing` is non-positive (no meaningful
/// count to divide by).
#[must_use]
#[expect(
    clippy::cast_sign_loss,
    reason = "the floor is checked non-negative via the length/spacing > 0.0 guard immediately above"
)]
#[expect(
    clippy::cast_possible_truncation,
    reason = "belt item counts are bounded well under usize::MAX for any realistic belt/spacing pair"
)]
pub fn belt_item_count(length: f64, spacing: f64) -> usize {
    if length <= 0.0 || spacing <= 0.0 {
        return 0;
    }
    (length / spacing).floor() as usize
}

/// The k-th evenly-spaced item's fractional position (`0.0..1.0`)
/// along a belt of `length` at time `t`, moving at `speed` with item
/// spacing `spacing`: `pos_k(t) = fract(speed*t/length -
/// k*spacing/length)` (shapez2 continuous-conveyor even distribution).
/// Stateless: every call recomputes fresh from its arguments, no
/// stored item list. `0.0` when `length` is non-positive (no belt to
/// place a fraction along).
#[must_use]
#[expect(
    clippy::cast_precision_loss,
    reason = "belt item indices are bounded well under 2^52 for any realistic belt/spacing pair"
)]
pub fn belt_item_phase(k: usize, t: f64, speed: f64, length: f64, spacing: f64) -> f64 {
    if length <= 0.0 {
        return 0.0;
    }
    let raw = speed.mul_add(t, -(k as f64 * spacing)) / length;
    raw.rem_euclid(1.0)
}

/// A [`crate::sim::WorkQueue`]-style bounded stock's fill ratio
/// (`0.0..=1.0`) for a queue-accumulation gauge: `level / capacity`,
/// clamped to `[0.0, 1.0]`. `0.0` when `capacity` is non-positive (no
/// meaningful ratio to divide by).
#[must_use]
pub fn fill_fraction(level: f64, capacity: f64) -> f64 {
    if capacity <= 0.0 {
        return 0.0;
    }
    (level / capacity).clamp(0.0, 1.0)
}

/// A bottom-anchored fill-rect's `(y, height)` in scene coordinate
/// space for a fraction-filled gauge (e.g. [`fill_fraction`]'s
/// output): `origin.1 + (box_height - fill_height)` and
/// `box_height * fraction`. The renderer's only source for the fill
/// rect's position and size; it must never derive them itself —
/// mirrors [`Scene::node_origin`]'s pass-through pattern.
#[must_use]
pub fn fill_rect_geometry(origin: (f64, f64), box_height: f64, fraction: f64) -> (f64, f64) {
    let fill_height = box_height * fraction;
    (origin.1 + (box_height - fill_height), fill_height)
}

/// One tick's update to a belt's live "activity" speed signal (feeds
/// [`belt_item_phase`]'s `speed` parameter): jumps to `boost` the tick
/// a matching sim event fires, otherwise decays `previous` toward
/// `floor` by `decay` (`0.0..1.0`) per tick — an EWMA-style falloff so
/// a belt visibly speeds up right after an arrival/dequeue and eases
/// back to its idle rate, rather than snapping. `floor` bounds the
/// decay so a belt never fully stalls.
#[must_use]
pub fn belt_activity_step(
    previous: f64,
    event_fired: bool,
    boost: f64,
    decay: f64,
    floor: f64,
) -> f64 {
    if event_fired {
        boost
    } else {
        (previous * decay).max(floor)
    }
}

/// One discrete pulse item carried by a [`BeltItemLog`]: the tick it
/// was spawned and the [`JobSource`] identity/color a stateless
/// [`belt_item_phase`] pattern cannot carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BeltItem {
    pub born_tick: u64,
    pub source: JobSource,
}

/// A bounded, prunable list of discrete [`BeltItem`]s for one belt —
/// the host-pure lift of the old DOM-child-count `MAX_LANE_PACKETS`
/// discipline (`prune_lane`) into a `VecDeque`, capped at
/// [`MAX_BELT_ITEMS`] by default.
#[derive(Debug, Clone)]
pub struct BeltItemLog {
    items: VecDeque<BeltItem>,
    capacity: usize,
}

impl BeltItemLog {
    /// Creates an empty log capped at [`MAX_BELT_ITEMS`].
    #[must_use]
    pub fn new() -> Self {
        Self::with_capacity(MAX_BELT_ITEMS)
    }

    /// Creates an empty log capped at `capacity` (clamped to at least
    /// `1`, mirroring [`crate::sd::LevelHistory::new`]'s convention).
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            items: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    /// Records `item` as the newest pulse. When already at capacity,
    /// the oldest retained item is evicted first.
    pub fn push(&mut self, item: BeltItem) {
        if self.items.len() == self.capacity {
            self.items.pop_front();
        }
        self.items.push_back(item);
    }

    /// Removes every item whose position (per [`Self::position_of`])
    /// at `now_tick` has reached or passed the belt's end (`>= 1.0`)
    /// — the pulse arrived and is no longer rendered.
    pub fn prune_finished(&mut self, now_tick: u64, speed: f64, length: f64) {
        self.items
            .retain(|item| Self::position(*item, now_tick, speed, length) < 1.0);
    }

    /// This item's fractional position (may exceed `1.0`, meaning it
    /// has already arrived) along a belt of `length` moving at
    /// `speed`, given it was born at `item.born_tick`.
    #[must_use]
    pub fn position_of(&self, item: &BeltItem, now_tick: u64, speed: f64, length: f64) -> f64 {
        Self::position(*item, now_tick, speed, length)
    }

    #[expect(
        clippy::cast_precision_loss,
        reason = "tick counts are bounded well under 2^52 for any realistic sim run"
    )]
    fn position(item: BeltItem, now_tick: u64, speed: f64, length: f64) -> f64 {
        if length <= 0.0 {
            return 0.0;
        }
        let elapsed = now_tick.saturating_sub(item.born_tick);
        speed * elapsed as f64 / length
    }

    /// Iterates retained items, oldest to newest.
    pub fn iter(&self) -> impl Iterator<Item = &BeltItem> {
        self.items.iter()
    }

    /// Number of items currently retained (never exceeds capacity).
    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// True when no items are currently retained.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The maximum number of items this log retains.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.capacity
    }
}

impl Default for BeltItemLog {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BeltItem, BeltItemLog, CloudRole, GhReportSceneError, GridSlot, NodeGeometry, Placement,
        Scene, ScenePlacementError, belt_activity_step, belt_item_count, belt_item_phase,
        fill_fraction, fill_rect_geometry, gh_report_scene,
    };
    use crate::layout::{self, GridParams, StockKind};
    use crate::sd::{Flow, Model, Stock, Terminal};
    use crate::sim::{JobSource, WebhookKind};

    const GRID: GridParams = GridParams {
        margin: 40.0,
        col_pitch: 205.0,
        row_pitch: 200.0,
        box_width: 180.0,
        box_height: 92.0,
    };

    fn tiny_model_and_placement() -> (Model, Placement) {
        let mut model = Model::new();
        let stock = model.add_stock(Stock::new(0.0));
        let cloud = model.add_cloud(Terminal::Source);
        model.connect_flow(cloud, stock, Flow::Uniflow(1.0));
        let placement = Placement::new(GRID)
            .with_stock(stock, GridSlot::new(1, 0), StockKind::Standard, "Stock")
            .with_cloud(cloud, GridSlot::new(0, 0), CloudRole::Source, "Cloud");
        (model, placement)
    }

    #[test]
    fn assemble_derives_one_belt_per_model_flow() {
        let (model, placement) = tiny_model_and_placement();
        let scene = Scene::assemble(model, &placement).expect("fully placed model assembles");
        assert_eq!(scene.belts().len(), scene.model().flow_count());
        assert_eq!(scene.nodes().len(), 2);
    }

    #[test]
    fn assemble_rejects_missing_slot_for_registered_stock() {
        let mut model = Model::new();
        let stock = model.add_stock(Stock::new(0.0));
        let cloud = model.add_cloud(Terminal::Source);
        model.connect_flow(cloud, stock, Flow::Uniflow(1.0));
        let placement =
            Placement::new(GRID).with_cloud(cloud, GridSlot::new(0, 0), CloudRole::Source, "Cloud");
        assert!(matches!(
            Scene::assemble(model, &placement),
            Err(ScenePlacementError::MissingSlot)
        ));
    }

    #[test]
    fn belt_endpoints_resolve_to_the_placed_slots_of_the_declared_model_endpoints() {
        let (model, placement) = tiny_model_and_placement();
        let scene = Scene::assemble(model, &placement).expect("assembles");
        let belt = &scene.belts()[0];
        let expected_from = layout::slot_anchor(0, 0, layout::Side::Bottom, GRID);
        let expected_to = layout::slot_anchor(1, 0, layout::Side::Top, GRID);
        assert_eq!(belt.from, expected_from);
        assert_eq!(belt.to, expected_to);
    }

    #[test]
    fn gh_report_scene_builds_ok() {
        let scene = gh_report_scene().expect("gh-report Tier-1 spine must place cleanly");
        assert_eq!(scene.model().stock_count(), 7);
        assert_eq!(scene.model().cloud_count(), 5);
        assert_eq!(scene.model().converter_count(), 3);
        assert_eq!(scene.belts().len(), 11);
        assert_eq!(scene.nodes().len(), 15);
    }

    #[test]
    fn gh_report_scene_no_two_nodes_overlap() {
        let scene = gh_report_scene().expect("scene builds");
        let origins: Vec<(f64, f64)> = scene
            .nodes()
            .iter()
            .map(|node| layout::grid_slot_origin(node.slot.row, node.slot.col, GRID))
            .collect();
        for (index, &(xa, ya)) in origins.iter().enumerate() {
            for &(xb, yb) in &origins[index + 1..] {
                let x_overlap = xa < xb + GRID.box_width && xb < xa + GRID.box_width;
                let y_overlap = ya < yb + GRID.box_height && yb < ya + GRID.box_height;
                assert!(
                    !(x_overlap && y_overlap),
                    "nodes overlap at ({xa},{ya})/({xb},{yb})"
                );
            }
        }
    }

    #[test]
    fn gh_report_scene_no_row_exceeds_six_nodes() {
        let scene = gh_report_scene().expect("scene builds");
        let mut counts = std::collections::HashMap::new();
        for node in scene.nodes() {
            *counts.entry(node.slot.row).or_insert(0) += 1;
        }
        assert!(counts.values().all(|&count: &usize| count <= 6));
    }

    #[test]
    fn gh_report_scene_sources_top_left_sinks_bottom_right() {
        let scene = gh_report_scene().expect("scene builds");
        let mut source_rows = Vec::new();
        let mut sink_rows = Vec::new();
        let mut other_rows = Vec::new();
        for node in scene.nodes() {
            match node.geometry {
                NodeGeometry::Cloud(_, CloudRole::Source) => source_rows.push(node.slot.row),
                NodeGeometry::Cloud(_, CloudRole::Sink) => sink_rows.push(node.slot.row),
                _ => other_rows.push(node.slot.row),
            }
        }
        let max_source_row = *source_rows.iter().max().expect("has source clouds");
        let min_sink_row = *sink_rows.iter().min().expect("has sink clouds");
        assert!(
            other_rows.iter().all(|&row| row > max_source_row),
            "every non-cloud node must sit strictly below every source cloud"
        );
        assert!(
            other_rows.iter().all(|&row| row < min_sink_row),
            "every non-cloud node must sit strictly above every sink cloud"
        );
    }

    #[test]
    fn gh_report_scene_every_belt_connects_a_declared_model_flow() {
        let scene = gh_report_scene().expect("scene builds");
        let model_flow_ids: std::collections::HashSet<_> =
            scene.model().flows().map(|view| view.id).collect();
        for belt in scene.belts() {
            assert!(
                model_flow_ids.contains(&belt.id),
                "belt {:?} must reference a FlowId the model actually registered",
                belt.id
            );
        }
        assert_eq!(scene.belts().len(), model_flow_ids.len());
    }

    #[test]
    fn gh_report_scene_error_is_non_exhaustive_and_debug_clone_eq() {
        let err = GhReportSceneError::MissingHandle;
        let cloned = err.clone();
        assert_eq!(err, cloned);
    }

    #[test]
    fn belt_item_count_is_floor_of_length_over_spacing() {
        assert_eq!(belt_item_count(100.0, 25.0), 4);
        assert_eq!(belt_item_count(101.0, 25.0), 4);
    }

    #[test]
    fn belt_item_count_zero_when_non_positive_inputs() {
        assert_eq!(belt_item_count(0.0, 25.0), 0);
        assert_eq!(belt_item_count(100.0, 0.0), 0);
        assert_eq!(belt_item_count(-5.0, 25.0), 0);
    }

    #[test]
    fn belt_item_phase_items_are_evenly_spaced_at_fixed_time() {
        let length = 100.0;
        let spacing = 25.0;
        let speed = 0.0;
        let phases: Vec<f64> = (0..4)
            .map(|k| belt_item_phase(k, 0.0, speed, length, spacing))
            .collect();
        for k in 0..phases.len() {
            let next = (k + 1) % phases.len();
            let raw_gap = (phases[k] - phases[next]).abs();
            let circular_gap = raw_gap.min(1.0 - raw_gap);
            assert!(
                (circular_gap - spacing / length).abs() < 1e-9,
                "adjacent items must be spacing/length apart on the ring, got gap {circular_gap}"
            );
        }
    }

    #[test]
    fn belt_item_phase_monotonic_per_tick_advance_before_wraparound() {
        let length = 100.0;
        let spacing = 25.0;
        let speed = 10.0;
        let mut previous = belt_item_phase(0, 0.0, speed, length, spacing);
        for tick in 1..5 {
            let current = belt_item_phase(0, f64::from(tick), speed, length, spacing);
            assert!(
                current > previous,
                "phase must advance monotonically before wraparound: tick {tick} gave {current}, previous {previous}"
            );
            previous = current;
        }
    }

    #[test]
    fn belt_item_phase_no_bunching_across_tick_boundary_wraparound() {
        let length = 100.0;
        let spacing = 25.0;
        let speed = 100.0;
        let phase_at_boundary = belt_item_phase(0, 1.0, speed, length, spacing);
        let phase_just_after = belt_item_phase(0, 1.001, speed, length, spacing);
        assert!(
            phase_at_boundary < 0.01,
            "wraparound must land near zero, got {phase_at_boundary}"
        );
        assert!(
            (phase_just_after - phase_at_boundary).abs() < 0.01,
            "no discontinuous jump ('bunching') across the wrap, got {phase_at_boundary} vs {phase_just_after}"
        );
    }

    #[test]
    fn belt_item_log_starts_empty() {
        let log = BeltItemLog::new();
        assert_eq!(log.len(), 0);
        assert!(log.is_empty());
        assert_eq!(log.capacity(), super::MAX_BELT_ITEMS);
    }

    #[test]
    fn belt_item_log_push_beyond_capacity_evicts_oldest() {
        let mut log = BeltItemLog::with_capacity(3);
        for tick in 0..13u64 {
            log.push(BeltItem {
                born_tick: tick,
                source: JobSource::ScheduledBatch,
            });
        }
        assert_eq!(log.len(), 3);
        let ticks: Vec<u64> = log.iter().map(|item| item.born_tick).collect();
        assert_eq!(ticks, vec![10, 11, 12], "only the 3 newest items survive");
    }

    #[test]
    fn belt_item_log_prune_finished_removes_arrived_items() {
        let mut log = BeltItemLog::new();
        log.push(BeltItem {
            born_tick: 0,
            source: JobSource::InitialLoad,
        });
        log.push(BeltItem {
            born_tick: 9,
            source: JobSource::External {
                id: 1,
                kind: WebhookKind::Push,
            },
        });
        log.prune_finished(10, 1.0, 5.0);
        assert_eq!(
            log.len(),
            1,
            "the item born at tick 0 (position 2.0) must be pruned"
        );
        assert_eq!(log.iter().next().expect("one item left").born_tick, 9);
    }

    #[test]
    fn belt_item_log_position_of_grows_with_elapsed_ticks() {
        let mut log = BeltItemLog::new();
        let item = BeltItem {
            born_tick: 0,
            source: JobSource::ScheduledBatch,
        };
        log.push(item);
        let early = log.position_of(&item, 1, 1.0, 10.0);
        let later = log.position_of(&item, 5, 1.0, 10.0);
        assert!(later > early);
    }

    #[test]
    fn node_origin_matches_layout_grid_slot_origin_for_the_nodes_slot() {
        let (model, placement) = tiny_model_and_placement();
        let scene = Scene::assemble(model, &placement).expect("assembles");
        for node in scene.nodes() {
            let expected = layout::grid_slot_origin(node.slot.row, node.slot.col, GRID);
            assert_eq!(scene.node_origin(node), expected);
        }
    }

    #[test]
    fn grid_returns_the_params_assemble_was_built_with() {
        let (model, placement) = tiny_model_and_placement();
        let scene = Scene::assemble(model, &placement).expect("assembles");
        assert_eq!(scene.grid(), GRID);
    }

    #[test]
    fn viewbox_dimensions_bounds_every_placed_node_box() {
        let scene = gh_report_scene().expect("scene builds");
        let (width, height) = scene.viewbox_dimensions();
        let grid = scene.grid();
        for node in scene.nodes() {
            let (x, y) = scene.node_origin(node);
            assert!(x + grid.box_width <= width, "node exceeds viewbox width");
            assert!(y + grid.box_height <= height, "node exceeds viewbox height");
        }
    }

    #[test]
    fn belt_point_at_endpoints_matches_from_and_to() {
        let (model, placement) = tiny_model_and_placement();
        let scene = Scene::assemble(model, &placement).expect("assembles");
        let belt = &scene.belts()[0];
        let start = belt.point_at(0.0);
        let end = belt.point_at(1.0);
        assert!((start.0 - belt.from.0).abs() < f64::EPSILON);
        assert!((end.0 - belt.to.0).abs() < f64::EPSILON);
    }

    #[test]
    fn belt_kind_matches_the_model_flows_registered_kind() {
        let (model, placement) = tiny_model_and_placement();
        let scene = Scene::assemble(model, &placement).expect("assembles");
        assert_eq!(scene.belts()[0].kind, Flow::Uniflow(1.0));
    }

    #[test]
    fn fill_fraction_is_level_over_capacity_clamped() {
        assert!((fill_fraction(8.0, 32.0) - 0.25).abs() < 1e-9);
        assert!(
            (fill_fraction(40.0, 32.0) - 1.0).abs() < 1e-9,
            "overfull clamps to 1.0"
        );
        assert!(
            fill_fraction(-5.0, 32.0).abs() < 1e-9,
            "negative level clamps to 0.0"
        );
    }

    #[test]
    fn fill_fraction_zero_when_capacity_non_positive() {
        assert!(fill_fraction(5.0, 0.0).abs() < 1e-9);
        assert!(fill_fraction(5.0, -1.0).abs() < 1e-9);
    }

    #[test]
    fn fill_rect_geometry_full_fraction_anchors_to_origin_top() {
        let (fill_y, fill_height) = fill_rect_geometry((10.0, 20.0), 92.0, 1.0);
        assert!(
            (fill_y - 20.0).abs() < 1e-9,
            "fraction=1.0 anchors to origin.1"
        );
        assert!((fill_height - 92.0).abs() < 1e-9);
    }

    #[test]
    fn fill_rect_geometry_zero_fraction_anchors_below_the_box() {
        let (fill_y, fill_height) = fill_rect_geometry((10.0, 20.0), 92.0, 0.0);
        assert!(
            (fill_y - (20.0 + 92.0)).abs() < 1e-9,
            "fraction=0.0 anchors to origin.1 + box_height"
        );
        assert!(fill_height.abs() < 1e-9);
    }

    #[test]
    fn fill_rect_geometry_half_fraction_is_bottom_anchored_midpoint() {
        let (fill_y, fill_height) = fill_rect_geometry((10.0, 20.0), 92.0, 0.5);
        assert!((fill_height - 46.0).abs() < 1e-9);
        assert!((fill_y - (20.0 + 46.0)).abs() < 1e-9);
    }

    #[test]
    fn belt_activity_step_jumps_to_boost_when_event_fires() {
        let next = belt_activity_step(2.0, true, 40.0, 0.85, 6.0);
        assert!((next - 40.0).abs() < 1e-9);
    }

    #[test]
    fn belt_activity_step_decays_toward_floor_without_an_event() {
        let after_one = belt_activity_step(40.0, false, 40.0, 0.5, 6.0);
        assert!((after_one - 20.0).abs() < 1e-9);
        let after_many = (0..20).fold(40.0, |value, _| {
            belt_activity_step(value, false, 40.0, 0.5, 6.0)
        });
        assert!(
            (after_many - 6.0).abs() < 1e-9,
            "decay never drops below floor"
        );
    }
}
