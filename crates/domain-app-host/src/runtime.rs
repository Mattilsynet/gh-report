//! Host runtime: once-compiled [`Component`] + [`Engine`], fresh
//! [`Store`] per call, under enforced deterministic resource limits.

use std::fmt;

use wasmtime::component::{Component, HasSelf, Linker, ResourceTable};
use wasmtime::{Config, Engine, ResourceLimiter, Store, StoreLimits, StoreLimitsBuilder};

use crate::error::HostError;
use crate::{CounterCommand, CounterEvent, CounterState, DomainApp, HandleError};

/// Per-call WASM fuel budget granted to the guest before the host traps
/// the call as [`HostError::FuelExhausted`].
///
/// # Determinism (G-G)
///
/// This is a compile-time `const`, deliberately never exposed as a
/// config-tunable or environment variable. Epoch-based interruption is
/// wall-clock driven, so two hosts replaying the same event stream could
/// trap at different points and diverge — the whole reason fuel was
/// chosen over epoch for this seam (SEC-0013:R3). Fuel *is* deterministic
/// given an identical limit, but only given an identical limit: making
/// the limit tunable would silently reopen the same cross-host replay
/// divergence hazard through the back door. Every host embedding this
/// crate observes exactly this many fuel units per call, with no
/// override path.
pub const CALL_FUEL_LIMIT: u64 = 10_000_000;

/// Per-call linear memory ceiling (SEC-0013:R3, SEC-0003): an
/// unconfigured instance is forbidden, so every store carries an
/// explicit memory limit alongside the fuel budget.
const MEMORY_LIMIT_BYTES: usize = 16 * 1024 * 1024;

/// Per-call aggregate store-owned allocation ceiling (SEC-0013:R3,
/// SEC-0003), separate and independent from [`MEMORY_LIMIT_BYTES`]: a
/// linear-memory cap alone bounds only wasm linear memory growth, not
/// table growth or other store-owned allocation, so this crate tracks
/// aggregate growth across both memories and tables via
/// [`AggregateLimiter`] rather than relying on `StoreLimits::memory_size`
/// alone to stand in for both bounds.
const STORE_SIZE_LIMIT_BYTES: usize = 32 * 1024 * 1024;

/// Maximum number of entries [`HostState::resources`] may ever hold
/// ([`wasmtime::component::ResourceTable::set_max_capacity`]).
///
/// # Why zero is the correct bound for this world today
///
/// The `domain-app` WIT world declares no `resource` type anywhere —
/// `counter-state`/`counter-command`/`counter-event`/`handle-error` are
/// all plain records/variants — so nothing in this world's public
/// interface can hand the guest a resource handle to create or hold.
/// The only interfaces this crate links (`wasi:io/error`, `wasi:io/poll`,
/// `wasi:io/streams`) exist purely to satisfy `wasm32-wasip2` libstd's
/// mandatory link-time baseline (SEC-0013:R1 footnote); a compute-only
/// guest like the reference `counter` aggregate never calls a function
/// that constructs a pollable or stream at runtime, so it never needs a
/// resource-table entry. A [`wasmtime::component::ResourceTable`] grows
/// independently of wasm linear memory and table growth, so
/// [`AggregateLimiter`] alone cannot bound it — this is a second,
/// independent, structural bound standing in for the SEC-0013:R3
/// aggregate store-owned-allocation cap on the one class of allocation
/// `AggregateLimiter` cannot see. If a future `domain-app` world
/// declares its own `resource` type, or a guest genuinely needs to hold
/// a stream/pollable across a call, this cap must be raised
/// deliberately — never silently, since it is this crate's only defence
/// against that allocation class.
const RESOURCE_TABLE_CAPACITY: usize = 0;

/// Distinct error raised when the host's [`MEMORY_LIMIT_BYTES`]
/// pre-check rejects a guest linear-memory growth request before ever
/// delegating to the inner [`StoreLimits`], kept separate from a
/// wasm-level trap so [`classify_trap`] can tell "the host's memory
/// budget was exceeded" apart from "the guest genuinely trapped"
/// (constraint 11, G-E). Needed because `StoreLimits::memory_growing`
/// under `trap_on_grow_failure(true)` raises a generic, undifferentiated
/// error rather than a `wasmtime::Trap::AllocationTooLarge` /
/// `MemoryOutOfBounds` variant — this type gives the rejection a stable
/// identity `classify_trap` can `downcast_ref` for.
#[derive(Debug)]
struct MemoryLimitExceeded;

impl fmt::Display for MemoryLimitExceeded {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "guest linear memory growth exceeded the {MEMORY_LIMIT_BYTES}-byte cap"
        )
    }
}

impl std::error::Error for MemoryLimitExceeded {}

/// Distinct error raised when [`AggregateLimiter`] rejects a memory or
/// table growth request because it would exceed
/// [`STORE_SIZE_LIMIT_BYTES`], kept separate from a wasm-level trap so
/// [`classify_trap`] can tell "the host's resource budget was exceeded"
/// apart from "the guest genuinely trapped" (constraint 11, G-E).
#[derive(Debug)]
struct StoreBudgetExceeded;

impl fmt::Display for StoreBudgetExceeded {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "store-owned allocation exceeded its {STORE_SIZE_LIMIT_BYTES}-byte budget"
        )
    }
}

impl std::error::Error for StoreBudgetExceeded {}

/// A [`ResourceLimiter`] that enforces two independent bounds per call:
/// the per-memory cap delegated to an inner [`StoreLimits`]
/// (`memory_size`), and a separate aggregate cap
/// ([`STORE_SIZE_LIMIT_BYTES`]) covering the sum of memory and table
/// growth this store has performed. `StoreLimits::memory_size` alone
/// bounds only a single linear memory, not table growth or aggregate
/// store-owned allocation (SEC-0013:R3 requires both a memory limit AND
/// a store-size cap, not one standing in for the other).
///
/// Growth rejected by either bound raises an `Err`, which Wasmtime
/// always turns into a trap regardless of `trap_on_grow_failure`
/// (constraint 6: an unconfigured, silently-degrading instance is
/// forbidden) — exhaustion is a loud, deterministic outcome, never a
/// guest-observable failed `memory.grow`/`table.grow` return value.
struct AggregateLimiter {
    inner: StoreLimits,
    bytes_used: usize,
}

impl AggregateLimiter {
    fn new() -> Self {
        Self {
            inner: StoreLimitsBuilder::new()
                .memory_size(MEMORY_LIMIT_BYTES)
                .instances(1)
                .tables(1)
                .memories(1)
                .trap_on_grow_failure(true)
                .build(),
            bytes_used: 0,
        }
    }

    fn charge(&mut self, additional_bytes: usize) -> wasmtime::Result<()> {
        let projected = self.bytes_used.saturating_add(additional_bytes);
        if projected > STORE_SIZE_LIMIT_BYTES {
            return Err(wasmtime::Error::new(StoreBudgetExceeded));
        }
        self.bytes_used = projected;
        Ok(())
    }
}

impl ResourceLimiter for AggregateLimiter {
    fn memory_growing(
        &mut self,
        current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        if desired > MEMORY_LIMIT_BYTES {
            return Err(wasmtime::Error::new(MemoryLimitExceeded));
        }
        self.charge(desired.saturating_sub(current))?;
        self.inner.memory_growing(current, desired, maximum)
    }

    fn table_growing(
        &mut self,
        current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        let element_bytes = size_of::<usize>();
        self.charge(desired.saturating_sub(current).saturating_mul(element_bytes))?;
        self.inner.table_growing(current, desired, maximum)
    }

    fn instances(&self) -> usize {
        self.inner.instances()
    }

    fn tables(&self) -> usize {
        self.inner.tables()
    }

    fn memories(&self) -> usize {
        self.inner.memories()
    }
}

struct HostState {
    resources: ResourceTable,
    limiter: AggregateLimiter,
}

/// A once-compiled `domain-app` component, ready to instantiate a fresh,
/// resource-bounded store per call (G-C).
pub struct HostRuntime {
    engine: Engine,
    component: Component,
    linker: Linker<HostState>,
}

impl HostRuntime {
    /// Compiles `component_bytes` once and builds a [`Linker`] that
    /// grants the guest nothing beyond the mandatory `wasi:io/error`,
    /// `wasi:io/poll`, and `wasi:io/streams` baseline `wasm32-wasip2`'s
    /// libstd always requires to instantiate (SEC-0013:R1 footnote) — no
    /// clocks, random, filesystem, network, or CLI capability is wired,
    /// and no [`wasmtime_wasi::WasiCtx`] is constructed at all: this
    /// crate deliberately does not call
    /// [`wasmtime_wasi::p2::add_to_linker_sync`], whose 47.0.3
    /// implementation additionally links wall/monotonic clocks, all
    /// three random interfaces, CLI, filesystem, and socket interfaces
    /// regardless of what the guest's WIT world declares.
    ///
    /// # Errors
    ///
    /// Returns [`HostError::ComponentLoad`] if `component_bytes` fails to
    /// parse or validate, or [`HostError::Instantiate`] if building the
    /// linker fails.
    pub fn new(component_bytes: &[u8]) -> Result<Self, HostError> {
        let mut config = Config::new();
        config.wasm_component_model(true);
        #[expect(
            deprecated,
            reason = "G-F: explicit sync-seam intent (PGN-0010:R5) kept even though wasmtime 47 made this call a no-op; sync-only usage is what actually enforces the seam"
        )]
        config.async_support(false);
        config.consume_fuel(true);
        let engine = Engine::new(&config).map_err(HostError::ComponentLoad)?;
        let component =
            Component::new(&engine, component_bytes).map_err(HostError::ComponentLoad)?;

        let mut linker = Linker::new(&engine);
        add_minimal_wasi_io(&mut linker).map_err(HostError::Instantiate)?;

        Ok(Self {
            engine,
            component,
            linker,
        })
    }

    fn new_store(&self) -> Store<HostState> {
        let mut resources = ResourceTable::new();
        resources.set_max_capacity(RESOURCE_TABLE_CAPACITY);
        let mut store = Store::new(
            &self.engine,
            HostState {
                resources,
                limiter: AggregateLimiter::new(),
            },
        );
        store.limiter(|state| &mut state.limiter);
        store
            .set_fuel(CALL_FUEL_LIMIT)
            .expect("fuel consumption enabled in Config::new");
        store
    }

    fn instantiate(&self, store: &mut Store<HostState>) -> Result<DomainApp, HostError> {
        DomainApp::instantiate(store, &self.component, &self.linker)
            .map_err(HostError::Instantiate)
    }

    /// Pure command handling (CHE-0008:R1/R2) over a fresh instance.
    ///
    /// # Errors
    ///
    /// Returns [`HostError::FuelExhausted`] or [`HostError::MemoryExhausted`]
    /// if the guest exceeds its resource budget, [`HostError::Instantiate`]
    /// on a linking failure, or [`HostError::HandleCommandTrapped`] if the
    /// guest traps rather than returning normally. The guest's own
    /// declared domain failure is the inner `Result::Err(HandleError)`.
    pub fn handle_command(
        &self,
        state: CounterState,
        cmd: CounterCommand,
    ) -> Result<Result<Vec<CounterEvent>, HandleError>, HostError> {
        let mut store = self.new_store();
        let app = self.instantiate(&mut store)?;
        app.call_handle_command(&mut store, state, cmd)
            .map_err(|err| classify_trap(err, TrapSite::HandleCommand))
    }

    /// Deterministic, total event application (CHE-0009:R1/R2) over a
    /// fresh instance. `apply-event` is specified as total and
    /// infallible: a trap here is a bug signal, kept in its own distinct
    /// [`HostError::ApplyEventTrapped`] variant, never swallowed or
    /// mapped onto a domain error (G-E).
    ///
    /// # Errors
    ///
    /// Returns [`HostError::FuelExhausted`] or [`HostError::MemoryExhausted`]
    /// if the guest exceeds its resource budget, [`HostError::Instantiate`]
    /// on a linking failure, or [`HostError::ApplyEventTrapped`] if the
    /// guest traps during what CHE-0009 specifies as a total function.
    pub fn apply_event(
        &self,
        state: CounterState,
        event: CounterEvent,
    ) -> Result<CounterState, HostError> {
        let mut store = self.new_store();
        let app = self.instantiate(&mut store)?;
        app.call_apply_event(&mut store, state, event)
            .map_err(|err| classify_trap(err, TrapSite::ApplyEvent))
    }
}

/// Which guest export a trapped call originated from, so
/// [`classify_trap`] can route an unclassified trap to the right
/// site-specific [`HostError`] variant without guessing.
#[derive(Clone, Copy)]
enum TrapSite {
    HandleCommand,
    ApplyEvent,
}

/// Classifies a failed guest call into a specific [`HostError`] variant.
///
/// Resolution order, most specific first:
/// 1. [`StoreBudgetExceeded`] (this crate's [`AggregateLimiter`]) or
///    [`MemoryLimitExceeded`] (the [`MEMORY_LIMIT_BYTES`] pre-check) ->
///    [`HostError::MemoryExhausted`] — both are this crate's own typed
///    rejections, distinguishable from any wasm-level trap code, and
///    both always surface as a trap by construction (`Err` from a
///    [`ResourceLimiter`] method always traps).
/// 2. [`wasmtime::Trap::OutOfFuel`] -> [`HostError::FuelExhausted`].
/// 3. [`wasmtime::Trap::MemoryOutOfBounds`] or
///    [`wasmtime::Trap::AllocationTooLarge`] (a genuine wasm-level
///    memory fault) -> [`HostError::MemoryExhausted`].
/// 4. Anything else -> the call-site-specific trap variant
///    ([`HostError::HandleCommandTrapped`] or
///    [`HostError::ApplyEventTrapped`]), never conflated with 1-3 — this
///    is what keeps an ordinary `apply-event` trap correctly reported as
///    a bug signal (CHE-0009, G-E) rather than laundered into
///    `MemoryExhausted`.
fn classify_trap(err: wasmtime::Error, site: TrapSite) -> HostError {
    if err.downcast_ref::<StoreBudgetExceeded>().is_some()
        || err.downcast_ref::<MemoryLimitExceeded>().is_some()
    {
        return HostError::MemoryExhausted;
    }
    if let Some(trap) = err.downcast_ref::<wasmtime::Trap>() {
        match trap {
            wasmtime::Trap::OutOfFuel => return HostError::FuelExhausted,
            wasmtime::Trap::AllocationTooLarge | wasmtime::Trap::MemoryOutOfBounds => {
                return HostError::MemoryExhausted;
            }
            _ => {}
        }
    }
    match site {
        TrapSite::HandleCommand => HostError::HandleCommandTrapped(err),
        TrapSite::ApplyEvent => HostError::ApplyEventTrapped(err),
    }
}

/// Links only the mandatory `wasi:io/error`, `wasi:io/poll`, and
/// `wasi:io/streams` interfaces `wasm32-wasip2`'s libstd requires to
/// instantiate (SEC-0013:R1 footnote) — no clocks, random, CLI,
/// filesystem, or socket interface is registered, and consequently no
/// [`wasmtime_wasi::WasiCtx`] needs to exist at all: [`HostState`]
/// carries only a [`ResourceTable`], never a `WasiCtx`, so there is no
/// clock or RNG for a guest to observe even accidentally.
fn add_minimal_wasi_io(linker: &mut Linker<HostState>) -> wasmtime::Result<()> {
    fn table(state: &mut HostState) -> &mut ResourceTable {
        &mut state.resources
    }
    wasmtime_wasi_io::bindings::wasi::io::error::add_to_linker::<HostState, HasSelf<ResourceTable>>(
        linker, table,
    )?;
    wasmtime_wasi::p2::bindings::sync::io::poll::add_to_linker::<HostState, HasSelf<ResourceTable>>(
        linker, table,
    )?;
    wasmtime_wasi::p2::bindings::sync::io::streams::add_to_linker::<
        HostState,
        HasSelf<ResourceTable>,
    >(linker, table)?;
    Ok(())
}
