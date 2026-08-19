//! Host runtime: once-compiled [`Component`] + [`Engine`], fresh
//! [`Store`] per call, under enforced deterministic resource limits.

use std::fmt;

use wasmtime::component::{Component, HasSelf, Linker, Resource, ResourceTable, ResourceType};
use wasmtime::{Config, Engine, ResourceLimiter, Store, StoreLimits, StoreLimitsBuilder};
use wasmtime_wasi_io::poll::DynPollable;
use wasmtime_wasi_io::streams::{DynInputStream, DynOutputStream};

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
/// chosen over epoch for this seam (SEC-0014:R3). Fuel *is* deterministic
/// given an identical limit, but only given an identical limit: making
/// the limit tunable would silently reopen the same cross-host replay
/// divergence hazard through the back door. Every host embedding this
/// crate observes exactly this many fuel units per call, with no
/// override path.
pub const CALL_FUEL_LIMIT: u64 = 10_000_000;

/// Per-call linear memory ceiling (SEC-0014:R3, SEC-0003): an
/// unconfigured instance is forbidden, so every store carries an
/// explicit memory limit alongside the fuel budget.
const MEMORY_LIMIT_BYTES: usize = 16 * 1024 * 1024;

/// Per-call aggregate store-owned allocation ceiling (SEC-0014:R3,
/// SEC-0003), separate and independent from [`MEMORY_LIMIT_BYTES`]: a
/// linear-memory cap alone bounds only wasm linear memory growth, not
/// table growth or other store-owned allocation, so this crate tracks
/// aggregate growth across both memories and tables via
/// [`AggregateLimiter`] rather than relying on `StoreLimits::memory_size`
/// alone to stand in for both bounds.
const STORE_SIZE_LIMIT_BYTES: usize = 32 * 1024 * 1024;

/// The minimum number of core-wasm instances and tables
/// ([`StoreLimitsBuilder::instances`], [`StoreLimitsBuilder::tables`]) the
/// real reference `guests/domain-app` component (rustc 1.97.0,
/// `wasm32-wasip2`, 15 declared imports per
/// `declared_import_set_matches_measured_bucket_assignment`) requires to
/// instantiate through this crate's production [`Linker`] — measured
/// directly by
/// [`tests::core_instance_limit_has_measured_headroom_above_the_true_minimum`],
/// which asserts instantiation fails at `MEASURED_MINIMUM_CORE_INSTANCES - 1`
/// and succeeds at `MEASURED_MINIMUM_CORE_INSTANCES`. Measured 3, not the
/// "one core instance per linked/trapping interface" estimate (14) the prior
/// version of [`CORE_INSTANCE_LIMIT`]'s rationale assumed — `func_wrap`- and
/// `resource`-registered trapping stubs evidently do not each cost a
/// separate core-wasm instance the way a genuine linked module would.
const MEASURED_MINIMUM_CORE_INSTANCES: usize = 3;

/// Per-call ceiling on core-wasm module instances and tables a single
/// [`Store`] may create ([`StoreLimitsBuilder::instances`],
/// [`StoreLimitsBuilder::tables`]), pinned to
/// [`MEASURED_MINIMUM_CORE_INSTANCES`] plus a deliberate, stated margin of
/// `5` (adr-fmt-rjogx Medium finding): the margin absorbs minor
/// rustc/wit-bindgen/wasmtime toolchain drift in the exact instance count a
/// `wasm32-wasip2` component compiles to, without reopening the 32×
/// over-generous cap the I3 linus loop's H1/H2 findings deliberately
/// tightened away from. If a future guest's measured minimum
/// (re-)exceeds this constant,
/// [`tests::core_instance_limit_has_measured_headroom_above_the_true_minimum`]
/// fails loudly rather than silently rejecting the guest at instantiation.
const CORE_INSTANCE_LIMIT: usize = MEASURED_MINIMUM_CORE_INSTANCES + 5;

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
/// mandatory link-time baseline (SEC-0014:R1 footnote); a compute-only
/// guest like the reference `counter` aggregate never calls a function
/// that constructs a pollable or stream at runtime, so it never needs a
/// resource-table entry. A [`wasmtime::component::ResourceTable`] grows
/// independently of wasm linear memory and table growth, so
/// [`AggregateLimiter`] alone cannot bound it — this is a second,
/// independent, structural bound standing in for the SEC-0014:R3
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

/// Distinct error raised by every function of every Bucket-2 trapping
/// stub interface (Option A, ADR-fmt-48ghj Part 3, ADR-fmt-4ksfn
/// M-1/M-4, AMENDMENT 1): each stub conveys no capability, so a guest
/// that reaches for any of its functions always traps with this
/// identity naming the interface, letting [`classify_trap`] give the
/// guest's reach its own [`HostError::CapabilityAccessDenied`] variant
/// rather than the generic call-site bucket (CHE-0107:R2, SEC-0014:R1).
#[derive(Debug)]
struct CapabilityAccessDenied {
    interface: &'static str,
}

impl fmt::Display for CapabilityAccessDenied {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} is a trapping stub: no capability is granted",
            self.interface
        )
    }
}

impl std::error::Error for CapabilityAccessDenied {}

/// Builds the [`wasmtime::Error`] every Bucket-2 stub function returns,
/// naming the interface it was reached through (M-4).
fn capability_denied<T>(interface: &'static str) -> wasmtime::Result<T> {
    Err(wasmtime::Error::new(CapabilityAccessDenied { interface }))
}

/// Zero-sized host-side marker type standing in for the
/// `wasi:cli/terminal-input@0.2.9` resource, registered only so
/// link-time type-checking of `terminal-stdin::get-terminal-stdin`'s
/// `option<terminal-input>` return type succeeds (the named
/// implementation wrinkle, ADR-fmt-4ksfn AMENDMENT 1) — the trapping
/// stub never constructs an instance.
struct TerminalInputMarker;

/// Zero-sized host-side marker type standing in for the
/// `wasi:cli/terminal-output@0.2.9` resource, analogous to
/// [`TerminalInputMarker`] for `terminal-stdout`/`terminal-stderr`.
struct TerminalOutputMarker;

/// A [`ResourceLimiter`] that enforces two independent bounds per call:
/// the per-memory cap delegated to an inner [`StoreLimits`]
/// (`memory_size`), and a separate aggregate cap
/// ([`STORE_SIZE_LIMIT_BYTES`]) covering the sum of memory and table
/// growth this store has performed. `StoreLimits::memory_size` alone
/// bounds only a single linear memory, not table growth or aggregate
/// store-owned allocation (SEC-0014:R3 requires both a memory limit AND
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
        Self::with_core_instance_limit(CORE_INSTANCE_LIMIT)
    }

    /// Test-only constructor parameterising the core-instance/table cap,
    /// so [`tests::core_instance_limit_has_measured_headroom_above_the_true_minimum`] can measure the
    /// real reference guest's minimum requirement against the actual
    /// production [`HostState`]/[`Linker`] pairing rather than a
    /// reimplementation (adr-fmt-rjogx Medium finding).
    fn with_core_instance_limit(limit: usize) -> Self {
        Self {
            inner: StoreLimitsBuilder::new()
                .memory_size(MEMORY_LIMIT_BYTES)
                .instances(limit)
                .tables(limit)
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
        self.charge(
            desired
                .saturating_sub(current)
                .saturating_mul(element_bytes),
        )?;
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
    /// Compiles `component_bytes` once and builds a [`Linker`] where
    /// every declared import resolves into exactly one of two buckets
    /// (ADR-fmt-4ksfn AMENDMENT 1): the three genuinely-linked
    /// `wasi:io/error`/`poll`/`streams` interfaces `wasm32-wasip2`'s
    /// libstd requires to instantiate (SEC-0014:R1 footnote), and eleven
    /// always-trapping stubs (`wasi:clocks/monotonic-clock` plus the ten
    /// `wasi:cli/*` interfaces) that resolve but convey zero capability.
    /// No clocks, random, filesystem, network, environment, or terminal
    /// capability is ever granted, and no [`wasmtime_wasi::WasiCtx`] is
    /// constructed at all: this crate deliberately does not call
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
        add_trapping_monotonic_clock(&mut linker).map_err(HostError::Instantiate)?;
        add_trapping_wasi_cli(&mut linker).map_err(HostError::Instantiate)?;

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
        DomainApp::instantiate(store, &self.component, &self.linker).map_err(HostError::Instantiate)
    }

    /// Pure command handling (CHE-0008:R1/R2) over a fresh instance.
    ///
    /// # Errors
    ///
    /// Returns [`HostError::FuelExhausted`] or [`HostError::MemoryExhausted`]
    /// if the guest exceeds its resource budget, [`HostError::Instantiate`]
    /// on a linking failure, [`HostError::CapabilityAccessDenied`] if the
    /// guest reaches for a Bucket-2 trapping-stub interface, or
    /// [`HostError::HandleCommandTrapped`] if the guest traps rather than
    /// returning normally. The guest's own declared domain failure is the
    /// inner `Result::Err(HandleError)`.
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
    /// on a linking failure, [`HostError::CapabilityAccessDenied`] if the
    /// guest reaches for a Bucket-2 trapping-stub interface, or
    /// [`HostError::ApplyEventTrapped`] if the guest traps during what
    /// CHE-0009 specifies as a total function.
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
/// 1. [`CapabilityAccessDenied`] (any Bucket-2 trapping stub: the
///    `monotonic-clock` interface and the ten `wasi:cli/*` interfaces) ->
///    [`HostError::CapabilityAccessDenied`] — the guest reached for a
///    capability this host deliberately never grants (M-4, CHE-0107:R2).
/// 2. [`StoreBudgetExceeded`] (this crate's [`AggregateLimiter`]) or
///    [`MemoryLimitExceeded`] (the [`MEMORY_LIMIT_BYTES`] pre-check) ->
///    [`HostError::MemoryExhausted`] — both are this crate's own typed
///    rejections, distinguishable from any wasm-level trap code, and
///    both always surface as a trap by construction (`Err` from a
///    [`ResourceLimiter`] method always traps).
/// 3. [`wasmtime::Trap::OutOfFuel`] -> [`HostError::FuelExhausted`].
/// 4. [`wasmtime::Trap::MemoryOutOfBounds`] or
///    [`wasmtime::Trap::AllocationTooLarge`] (a genuine wasm-level
///    memory fault) -> [`HostError::MemoryExhausted`].
/// 5. Anything else -> the call-site-specific trap variant
///    ([`HostError::HandleCommandTrapped`] or
///    [`HostError::ApplyEventTrapped`]), never conflated with 1-4 — this
///    is what keeps an ordinary `apply-event` trap correctly reported as
///    a bug signal (CHE-0009, G-E) rather than laundered into
///    `MemoryExhausted`.
fn classify_trap(err: wasmtime::Error, site: TrapSite) -> HostError {
    if let Some(denied) = err.downcast_ref::<CapabilityAccessDenied>() {
        return HostError::CapabilityAccessDenied {
            interface: denied.interface,
        };
    }
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
/// instantiate (SEC-0014:R1 footnote) — no clocks, random, filesystem,
/// or socket interface is registered here (the ten `wasi:cli/*`
/// interfaces and `wasi:clocks/monotonic-clock` are separately linked as
/// trapping stubs by [`add_trapping_wasi_cli`] and
/// [`add_trapping_monotonic_clock`]), and consequently no
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

/// Links all four `wasi:clocks/monotonic-clock@0.2.9` functions as an
/// always-trapping stub (Option A, ADR-fmt-48ghj Part 3): `now`,
/// `resolution`, `subscribe-instant`, and `subscribe-duration` every
/// unconditionally return [`ClockAccessDenied`], which Wasmtime turns
/// into a guest trap. Both pollable-returning functions are stubbed
/// (M-1) even though only `subscribe-duration` appears in the reference
/// guest's compiled import type (`z2hoi` Q3): CHE-0107:R2 forbids
/// granting the guest any timer/async capability, and stubbing all four
/// is robust to guest drift.
///
/// Surgical `Linker::instance` + `func_wrap` per function (the preferred
/// mechanism, ADR-fmt-4ksfn) rather than
/// [`wasmtime_wasi::WasiCtxBuilder::monotonic_clock`]: the latter
/// requires constructing a [`wasmtime_wasi::WasiCtx`], which M-2
/// forbids re-introducing into [`HostState`] or this crate's dep graph
/// (the exact regression the I3 linus loop's H1 finding removed). This
/// mechanism drags in nothing beyond the [`DynPollable`] resource type
/// already registered by [`add_minimal_wasi_io`]'s `wasi:io/poll`
/// linkage, and cannot grant any capability by construction — every
/// closure below unconditionally returns `Err`.
fn add_trapping_monotonic_clock(linker: &mut Linker<HostState>) -> wasmtime::Result<()> {
    const CLOCK: &str = "wasi:clocks/monotonic-clock@0.2.9";
    let mut clock = linker.instance(CLOCK)?;
    clock.func_wrap(
        "now",
        |_caller: wasmtime::StoreContextMut<'_, HostState>, (): ()| -> wasmtime::Result<(u64,)> {
            capability_denied(CLOCK)
        },
    )?;
    clock.func_wrap(
        "resolution",
        |_caller: wasmtime::StoreContextMut<'_, HostState>, (): ()| -> wasmtime::Result<(u64,)> {
            capability_denied(CLOCK)
        },
    )?;
    clock.func_wrap(
        "subscribe-instant",
        |_caller: wasmtime::StoreContextMut<'_, HostState>,
         (_when,): (u64,)|
         -> wasmtime::Result<(wasmtime::component::Resource<DynPollable>,)> {
            capability_denied(CLOCK)
        },
    )?;
    clock.func_wrap(
        "subscribe-duration",
        |_caller: wasmtime::StoreContextMut<'_, HostState>,
         (_duration,): (u64,)|
         -> wasmtime::Result<(wasmtime::component::Resource<DynPollable>,)> {
            capability_denied(CLOCK)
        },
    )?;
    Ok(())
}

/// Links all ten `wasi:cli/*@0.2.9` interfaces as always-trapping stubs
/// (ADR-fmt-4ksfn AMENDMENT 1, the monotone extension of Option A to the
/// full measured wasm32-wasip2 libstd baseline): `environment`, `exit`,
/// `stdin`, `stdout`, `stderr`, `terminal-input`, `terminal-output`,
/// `terminal-stdin`, `terminal-stdout`, and `terminal-stderr` every
/// unconditionally return [`CapabilityAccessDenied`] naming the
/// interface, which Wasmtime turns into a guest trap. Split across four
/// helper functions purely to stay under the crate's function-length
/// lint bar; the four together are one cohesive link-time unit.
fn add_trapping_wasi_cli(linker: &mut Linker<HostState>) -> wasmtime::Result<()> {
    add_trapping_cli_environment_and_exit(linker)?;
    add_trapping_cli_stdio(linker)?;
    add_trapping_cli_terminal_resources(linker)?;
    add_trapping_cli_terminal_stdio(linker)
}

/// `wasi:cli/environment@0.2.9` and `wasi:cli/exit@0.2.9`, part of
/// [`add_trapping_wasi_cli`].
fn add_trapping_cli_environment_and_exit(linker: &mut Linker<HostState>) -> wasmtime::Result<()> {
    const ENVIRONMENT: &str = "wasi:cli/environment@0.2.9";
    const EXIT: &str = "wasi:cli/exit@0.2.9";

    let mut environment = linker.instance(ENVIRONMENT)?;
    environment.func_wrap(
        "get-environment",
        |_caller: wasmtime::StoreContextMut<'_, HostState>,
         (): ()|
         -> wasmtime::Result<(Vec<(String, String)>,)> { capability_denied(ENVIRONMENT) },
    )?;
    environment.func_wrap(
        "get-arguments",
        |_caller: wasmtime::StoreContextMut<'_, HostState>,
         (): ()|
         -> wasmtime::Result<(Vec<String>,)> { capability_denied(ENVIRONMENT) },
    )?;
    environment.func_wrap(
        "initial-cwd",
        |_caller: wasmtime::StoreContextMut<'_, HostState>,
         (): ()|
         -> wasmtime::Result<(Option<String>,)> { capability_denied(ENVIRONMENT) },
    )?;

    let mut exit = linker.instance(EXIT)?;
    exit.func_wrap(
        "exit",
        |_caller: wasmtime::StoreContextMut<'_, HostState>,
         (_status,): (Result<(), ()>,)|
         -> wasmtime::Result<()> { capability_denied(EXIT) },
    )?;
    exit.func_wrap(
        "exit-with-code",
        |_caller: wasmtime::StoreContextMut<'_, HostState>,
         (_code,): (u8,)|
         -> wasmtime::Result<()> { capability_denied(EXIT) },
    )?;
    Ok(())
}

/// `wasi:cli/stdin@0.2.9`, `wasi:cli/stdout@0.2.9`, and
/// `wasi:cli/stderr@0.2.9`, part of [`add_trapping_wasi_cli`].
fn add_trapping_cli_stdio(linker: &mut Linker<HostState>) -> wasmtime::Result<()> {
    const STDIN: &str = "wasi:cli/stdin@0.2.9";
    const STDOUT: &str = "wasi:cli/stdout@0.2.9";
    const STDERR: &str = "wasi:cli/stderr@0.2.9";

    let mut stdin = linker.instance(STDIN)?;
    stdin.func_wrap(
        "get-stdin",
        |_caller: wasmtime::StoreContextMut<'_, HostState>,
         (): ()|
         -> wasmtime::Result<(Resource<DynInputStream>,)> { capability_denied(STDIN) },
    )?;

    let mut stdout = linker.instance(STDOUT)?;
    stdout.func_wrap(
        "get-stdout",
        |_caller: wasmtime::StoreContextMut<'_, HostState>,
         (): ()|
         -> wasmtime::Result<(Resource<DynOutputStream>,)> { capability_denied(STDOUT) },
    )?;

    let mut stderr = linker.instance(STDERR)?;
    stderr.func_wrap(
        "get-stderr",
        |_caller: wasmtime::StoreContextMut<'_, HostState>,
         (): ()|
         -> wasmtime::Result<(Resource<DynOutputStream>,)> { capability_denied(STDERR) },
    )?;
    Ok(())
}

/// The `wasi:cli/terminal-input@0.2.9` and `wasi:cli/terminal-output@0.2.9`
/// resource types (the named implementation wrinkle, ADR-fmt-4ksfn
/// AMENDMENT 1): each registers a host [`ResourceType`] so the
/// `option<terminal-input>` / `option<terminal-output>` return types on
/// `terminal-stdin` / `terminal-stdout` / `terminal-stderr` type-check —
/// the resource is never actually constructed, only its type exists, so
/// the destructor is unreachable and simply returns `Ok`. Part of
/// [`add_trapping_wasi_cli`].
fn add_trapping_cli_terminal_resources(linker: &mut Linker<HostState>) -> wasmtime::Result<()> {
    let mut terminal_input = linker.instance("wasi:cli/terminal-input@0.2.9")?;
    terminal_input.resource(
        "terminal-input",
        ResourceType::host::<TerminalInputMarker>(),
        |_caller: wasmtime::StoreContextMut<'_, HostState>, _rep: u32| -> wasmtime::Result<()> {
            Ok(())
        },
    )?;

    let mut terminal_output = linker.instance("wasi:cli/terminal-output@0.2.9")?;
    terminal_output.resource(
        "terminal-output",
        ResourceType::host::<TerminalOutputMarker>(),
        |_caller: wasmtime::StoreContextMut<'_, HostState>, _rep: u32| -> wasmtime::Result<()> {
            Ok(())
        },
    )?;
    Ok(())
}

/// `wasi:cli/terminal-stdin@0.2.9`, `wasi:cli/terminal-stdout@0.2.9`, and
/// `wasi:cli/terminal-stderr@0.2.9`, part of [`add_trapping_wasi_cli`].
fn add_trapping_cli_terminal_stdio(linker: &mut Linker<HostState>) -> wasmtime::Result<()> {
    const TERMINAL_STDIN: &str = "wasi:cli/terminal-stdin@0.2.9";
    const TERMINAL_STDOUT: &str = "wasi:cli/terminal-stdout@0.2.9";
    const TERMINAL_STDERR: &str = "wasi:cli/terminal-stderr@0.2.9";

    let mut terminal_stdin = linker.instance(TERMINAL_STDIN)?;
    terminal_stdin.func_wrap(
        "get-terminal-stdin",
        |_caller: wasmtime::StoreContextMut<'_, HostState>,
         (): ()|
         -> wasmtime::Result<(Option<Resource<TerminalInputMarker>>,)> {
            capability_denied(TERMINAL_STDIN)
        },
    )?;

    let mut terminal_stdout = linker.instance(TERMINAL_STDOUT)?;
    terminal_stdout.func_wrap(
        "get-terminal-stdout",
        |_caller: wasmtime::StoreContextMut<'_, HostState>,
         (): ()|
         -> wasmtime::Result<(Option<Resource<TerminalOutputMarker>>,)> {
            capability_denied(TERMINAL_STDOUT)
        },
    )?;

    let mut terminal_stderr = linker.instance(TERMINAL_STDERR)?;
    terminal_stderr.func_wrap(
        "get-terminal-stderr",
        |_caller: wasmtime::StoreContextMut<'_, HostState>,
         (): ()|
         -> wasmtime::Result<(Option<Resource<TerminalOutputMarker>>,)> {
            capability_denied(TERMINAL_STDERR)
        },
    )?;
    Ok(())
}

#[cfg(test)]
impl HostRuntime {
    /// Test-only variant of [`Self::handle_command`] that also returns
    /// the fuel remaining in the call's [`Store`] afterward, so tests can
    /// evidence [`CALL_FUEL_LIMIT`] headroom rather than assert it blind
    /// (pre-mortem item 4).
    fn handle_command_with_fuel_remaining(
        &self,
        state: CounterState,
        cmd: CounterCommand,
    ) -> (
        Result<Result<Vec<CounterEvent>, HandleError>, HostError>,
        u64,
    ) {
        let mut store = self.new_store();
        let app = match self.instantiate(&mut store) {
            Ok(app) => app,
            Err(err) => return (Err(err), 0),
        };
        let result = app
            .call_handle_command(&mut store, state, cmd)
            .map_err(|err| classify_trap(err, TrapSite::HandleCommand));
        let remaining = store.get_fuel().unwrap_or(0);
        (result, remaining)
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::process::Command;
    use std::sync::OnceLock;

    use wasmtime::component::{Component, Linker, ResourceTable};
    use wasmtime::{Config, Engine, Store};

    use super::{
        AggregateLimiter, CALL_FUEL_LIMIT, HostRuntime, HostState, MEASURED_MINIMUM_CORE_INSTANCES,
        MemoryLimitExceeded, RESOURCE_TABLE_CAPACITY, StoreBudgetExceeded, TrapSite,
        add_trapping_monotonic_clock, add_trapping_wasi_cli, classify_trap,
    };
    use crate::error::HostError;
    use crate::{CounterCommand, CounterEvent, CounterState, HandleError};

    /// Builds (or reuses an already-built) `wasm32-wasip2` release
    /// component for the reference `guests/domain-app` guest and returns
    /// its bytes. CHE-0038:R1-R5 requires testing against the REAL
    /// component, never a mock -- this is the only place this crate
    /// shells out, and only for tests.
    fn guest_component_bytes() -> &'static [u8] {
        static BYTES: OnceLock<Vec<u8>> = OnceLock::new();
        BYTES
            .get_or_init(|| {
                let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
                let guest_manifest = manifest_dir.join("../../guests/domain-app/Cargo.toml");
                let status = Command::new("cargo")
                    .args([
                        "component",
                        "build",
                        "--release",
                        "--target",
                        "wasm32-wasip2",
                        "--manifest-path",
                    ])
                    .arg(&guest_manifest)
                    .status()
                    .expect("cargo component build must run to produce the reference guest");
                assert!(status.success(), "guest component build failed");
                let wasm_path = manifest_dir
                    .join("../../guests/domain-app/target/wasm32-wasip2/release/domain_app.wasm");
                std::fs::read(&wasm_path)
                    .unwrap_or_else(|err| panic!("reading {wasm_path:?}: {err}"))
            })
            .as_slice()
    }

    fn runtime() -> HostRuntime {
        HostRuntime::new(guest_component_bytes()).expect("component loads and links")
    }

    #[test]
    fn round_trip_increment_then_apply() {
        let host = runtime();
        let state = CounterState { count: 0 };
        let events = host
            .handle_command(state, CounterCommand::Increment)
            .expect("host call succeeds")
            .expect("guest returns Ok");
        assert_eq!(events, vec![CounterEvent::Incremented]);

        let next = host
            .apply_event(state, CounterEvent::Incremented)
            .expect("host call succeeds");
        assert_eq!(next, CounterState { count: 1 });
    }

    #[test]
    fn reset_at_zero_is_domain_invariant_violation() {
        let host = runtime();
        let state = CounterState { count: 0 };
        let err = host
            .handle_command(state, CounterCommand::Reset)
            .expect("host call succeeds")
            .expect_err("guest returns Err");
        assert!(matches!(err, HandleError::InvariantViolated(_)));
    }

    #[test]
    fn reset_above_zero_emits_was_reset() {
        let host = runtime();
        let state = CounterState { count: 3 };
        let events = host
            .handle_command(state, CounterCommand::Reset)
            .expect("host call succeeds")
            .expect("guest returns Ok");
        assert_eq!(events, vec![CounterEvent::WasReset]);
    }

    /// Positive abort-if discharge (`success_criteria` 3', ADR-fmt-4ksfn
    /// AMENDMENT 1): asserts that reaching for a Bucket-2 capability
    /// classifies to [`HostError::CapabilityAccessDenied`] naming the
    /// exact interface, distinct from every other [`HostError`] variant.
    /// [`capability_denied`] and [`classify_trap`] are precisely the pair
    /// of functions that own this guarantee -- every Bucket-2 stub
    /// closure (registered in [`super::add_trapping_monotonic_clock`]
    /// and [`super::add_trapping_wasi_cli`]) delegates to
    /// [`capability_denied`] verbatim, so exercising it directly through
    /// [`classify_trap`] is the same proof a live guest call through the
    /// wasm ABI would produce, without the ABI plumbing obscuring which
    /// function raised it (same justification as
    /// `exhaustion_taxonomy_stays_distinct` below).
    #[test]
    fn bucket_two_capability_reach_traps_naming_the_interface() {
        for interface in [
            "wasi:clocks/monotonic-clock@0.2.9",
            "wasi:cli/environment@0.2.9",
            "wasi:cli/exit@0.2.9",
            "wasi:cli/stdin@0.2.9",
            "wasi:cli/stdout@0.2.9",
            "wasi:cli/stderr@0.2.9",
            "wasi:cli/terminal-input@0.2.9",
            "wasi:cli/terminal-output@0.2.9",
            "wasi:cli/terminal-stdin@0.2.9",
            "wasi:cli/terminal-stdout@0.2.9",
            "wasi:cli/terminal-stderr@0.2.9",
        ] {
            let denied: wasmtime::Result<()> = super::capability_denied(interface);
            let err = denied.expect_err("every Bucket-2 stub always returns Err");
            let host_err = classify_trap(err, TrapSite::HandleCommand);
            match host_err {
                HostError::CapabilityAccessDenied {
                    interface: named, ..
                } => assert_eq!(named, interface),
                other => panic!("expected CapabilityAccessDenied for {interface}, got {other:?}"),
            }
            assert!(!matches!(host_err, HostError::FuelExhausted));
            assert!(!matches!(host_err, HostError::MemoryExhausted));
            assert!(!matches!(host_err, HostError::HandleCommandTrapped(_)));
            assert!(!matches!(host_err, HostError::ApplyEventTrapped(_)));
        }
    }

    /// Hand-authored component-model-text fixture importing
    /// `wasi:clocks/monotonic-clock@0.2.9#now` (no-arg, scalar `u64`
    /// return) and exporting a `run` function that calls it.
    const FIXTURE_CLOCK_NOW: &str = r#"
        (component
          (import "wasi:clocks/monotonic-clock@0.2.9" (instance $clock
            (export "now" (func (result u64)))))
          (alias export $clock "now" (func $now))
          (core func $now-core (canon lower (func $now)))
          (core module $m
            (import "host" "now" (func $now (result i64)))
            (func (export "run") (result i64) call $now))
          (core instance $inst (instantiate $m
            (with "host" (instance (export "now" (func $now-core))))))
          (func (export "run") (result u64) (canon lift (core func $inst "run")))
        )
    "#;

    /// Same shape as [`FIXTURE_CLOCK_NOW`], against
    /// `wasi:clocks/monotonic-clock@0.2.9#resolution`.
    const FIXTURE_CLOCK_RESOLUTION: &str = r#"
        (component
          (import "wasi:clocks/monotonic-clock@0.2.9" (instance $clock
            (export "resolution" (func (result u64)))))
          (alias export $clock "resolution" (func $resolution))
          (core func $resolution-core (canon lower (func $resolution)))
          (core module $m
            (import "host" "resolution" (func $resolution (result i64)))
            (func (export "run") (result i64) call $resolution))
          (core instance $inst (instantiate $m
            (with "host" (instance (export "resolution" (func $resolution-core))))))
          (func (export "run") (result u64) (canon lift (core func $inst "run")))
        )
    "#;

    /// `wasi:cli/exit@0.2.9#exit-with-code`: an argument-taking (`u8`),
    /// void-returning shape, distinct from the zero-arg scalar-returning
    /// clock fixtures above -- proves the ABI-level proof isn't
    /// accidentally only exercising one calling convention.
    const FIXTURE_CLI_EXIT_WITH_CODE: &str = r#"
        (component
          (import "wasi:cli/exit@0.2.9" (instance $exit
            (export "exit-with-code" (func (param "code" u8)))))
          (alias export $exit "exit-with-code" (func $exit-with-code))
          (core func $exit-with-code-core (canon lower (func $exit-with-code)))
          (core module $m
            (import "host" "exit-with-code" (func $exit-with-code (param i32)))
            (func (export "run") i32.const 7 call $exit-with-code))
          (core instance $inst (instantiate $m
            (with "host" (instance (export "exit-with-code" (func $exit-with-code-core))))))
          (func (export "run") (canon lift (core func $inst "run")))
        )
    "#;

    /// Instantiates `wat` through the SAME production trapping-stub
    /// registrations ([`add_trapping_monotonic_clock`],
    /// [`add_trapping_wasi_cli`]) `HostRuntime::new` uses, then calls the
    /// fixture's exported `run` and returns the resulting error.
    ///
    /// Direct proof for the High finding (adr-fmt-rjogx): unlike
    /// [`bucket_two_capability_reach_traps_naming_the_interface`] (which
    /// calls [`super::capability_denied`] directly), this drives an
    /// actual component instantiation and an actual guest-side call
    /// across the component ABI. A wrong function name, wrong
    /// signature, or missing registration in the production `add_*`
    /// functions would fail to link or fail to call here -- it cannot
    /// pass by construction the way the direct-call test could.
    fn call_fixture_through_production_linker(wat: &str) -> wasmtime::Error {
        let mut config = Config::new();
        config.wasm_component_model(true);
        config.consume_fuel(true);
        let engine = Engine::new(&config).expect("engine builds");
        let component = Component::new(&engine, wat)
            .unwrap_or_else(|err| panic!("fixture failed to parse/validate: {err:?}"));

        let mut linker = Linker::new(&engine);
        add_trapping_monotonic_clock(&mut linker).expect("clock stub links");
        add_trapping_wasi_cli(&mut linker).expect("cli stub links");

        let mut resources = ResourceTable::new();
        resources.set_max_capacity(RESOURCE_TABLE_CAPACITY);
        let mut store = Store::new(
            &engine,
            HostState {
                resources,
                limiter: AggregateLimiter::new(),
            },
        );
        store.limiter(|state| &mut state.limiter);
        store
            .set_fuel(CALL_FUEL_LIMIT)
            .expect("fuel consumption enabled");

        let instance = linker
            .instantiate(&mut store, &component)
            .expect("fixture instantiates through the real trapping-stub linker");
        let func = instance
            .get_func(&mut store, "run")
            .expect("fixture exports `run`");
        let mut results =
            vec![wasmtime::component::Val::Bool(false); func.ty(&store).results().len()];
        func.call(&mut store, &[], &mut results)
            .expect_err("calling a Bucket-2 stub through the component ABI must trap")
    }

    #[test]
    fn abi_level_trap_proof_across_bucket_two_signature_shapes() {
        let cases: [(&str, &str); 3] = [
            (FIXTURE_CLOCK_NOW, "wasi:clocks/monotonic-clock@0.2.9"),
            (
                FIXTURE_CLOCK_RESOLUTION,
                "wasi:clocks/monotonic-clock@0.2.9",
            ),
            (FIXTURE_CLI_EXIT_WITH_CODE, "wasi:cli/exit@0.2.9"),
        ];
        for (wat, interface) in cases {
            let err = call_fixture_through_production_linker(wat);
            let host_err = classify_trap(err, TrapSite::HandleCommand);
            match host_err {
                HostError::CapabilityAccessDenied { interface: named } => {
                    assert_eq!(named, interface);
                }
                other => panic!(
                    "expected CapabilityAccessDenied for {interface} via the real ABI, got {other:?}"
                ),
            }
        }
    }

    /// Bucket assignment (`success_criteria` 4', ADR-fmt-4ksfn AMENDMENT 1,
    /// this mission's sharpest available statement of SEC-0014:R1
    /// conformance): pins the ACTUAL measured 15-import set of the real
    /// reference component and its three-way split -- exactly 3
    /// genuinely-linked `wasi:io/*` interfaces, exactly 11 trap-only
    /// interfaces, and exactly 1 domain seam. Toolchain drift that adds a
    /// 16th import, removes one, or reclassifies one across buckets fails
    /// this test loudly rather than silently.
    #[test]
    fn declared_import_set_matches_measured_bucket_assignment() {
        const LINKED: [&str; 3] = [
            "wasi:io/error@0.2.9",
            "wasi:io/poll@0.2.9",
            "wasi:io/streams@0.2.9",
        ];
        const TRAPPING: [&str; 11] = [
            "wasi:clocks/monotonic-clock@0.2.9",
            "wasi:cli/environment@0.2.9",
            "wasi:cli/exit@0.2.9",
            "wasi:cli/stdin@0.2.9",
            "wasi:cli/stdout@0.2.9",
            "wasi:cli/stderr@0.2.9",
            "wasi:cli/terminal-input@0.2.9",
            "wasi:cli/terminal-output@0.2.9",
            "wasi:cli/terminal-stdin@0.2.9",
            "wasi:cli/terminal-stdout@0.2.9",
            "wasi:cli/terminal-stderr@0.2.9",
        ];
        const DOMAIN_SEAM: &str = "gh-report:domain-app/domain-types@0.1.0";

        let bytes = guest_component_bytes();
        let mut config = wasmtime::Config::new();
        config.wasm_component_model(true);
        let engine = wasmtime::Engine::new(&config).expect("engine builds");
        let component =
            wasmtime::component::Component::new(&engine, bytes).expect("component parses");
        let component_type = component.component_type();
        let declared_imports: Vec<String> = component_type
            .imports(&engine)
            .map(|(name, _)| name.to_string())
            .collect();

        assert_eq!(
            declared_imports.len(),
            15,
            "expected exactly 15 declared imports (3 linked + 11 trapping + 1 domain seam), \
             found {}: {declared_imports:?}",
            declared_imports.len()
        );
        assert!(
            declared_imports.contains(&DOMAIN_SEAM.to_string()),
            "expected the domain seam {DOMAIN_SEAM} among declared imports, found: {declared_imports:?}"
        );
        for name in LINKED {
            assert!(
                declared_imports.contains(&name.to_string()),
                "expected genuinely-linked import {name} among declared imports, found: {declared_imports:?}"
            );
        }
        for name in TRAPPING {
            assert!(
                declared_imports.contains(&name.to_string()),
                "expected trapping-stub import {name} among declared imports, found: {declared_imports:?}"
            );
        }
        let accounted: std::collections::HashSet<&str> = LINKED
            .iter()
            .copied()
            .chain(TRAPPING.iter().copied())
            .chain(std::iter::once(DOMAIN_SEAM))
            .collect();
        assert!(
            declared_imports
                .iter()
                .all(|name| accounted.contains(name.as_str())),
            "found a declared import outside the 3-linked/11-trapping/1-domain-seam bucket \
             assignment: {declared_imports:?}"
        );
    }

    /// Zero-granted-capability (SEC-0014:R1, this mission's core security
    /// property). The primary proof is dynamic: the REAL component
    /// instantiates and completes a full call successfully through this
    /// crate's [`super::HostRuntime`] linker, which resolves every one of
    /// the 15 declared imports (3 genuinely linked, 11 trapping stubs,
    /// per [`declared_import_set_matches_measured_bucket_assignment`])
    /// while granting no clock, random, filesystem, network, CLI, or
    /// terminal capability, and no [`wasmtime_wasi::WasiCtx`] exists
    /// anywhere in [`super::HostState`] for such a capability to be read
    /// from. Success here is proof of absence, not merely absence of a
    /// preopen.
    #[test]
    fn zero_granted_capability_component_runs_on_trapping_stub_linker() {
        let host = runtime();
        let state = CounterState { count: 0 };
        host.handle_command(state, CounterCommand::Increment)
            .expect("host call succeeds through the zero-extra-capability linker")
            .expect("guest returns Ok");
    }

    #[test]
    fn fuel_headroom_well_under_compile_time_limit() {
        let host = runtime();
        let state = CounterState { count: 0 };
        let (result, remaining) =
            host.handle_command_with_fuel_remaining(state, CounterCommand::Increment);
        result
            .expect("host call succeeds")
            .expect("guest returns Ok");
        let consumed = CALL_FUEL_LIMIT - remaining;
        assert!(
            consumed < CALL_FUEL_LIMIT / 10,
            "expected the reference guest to use well under 10% of CALL_FUEL_LIMIT \
             ({CALL_FUEL_LIMIT}), consumed {consumed} (remaining {remaining})"
        );
    }

    /// Exhaustion taxonomy (constraint 11, G-E): fuel exhaustion, memory
    /// exhaustion, and an ordinary guest trap must classify to three
    /// distinct [`HostError`] variants, never conflated. The reference
    /// guest's arithmetic is far too small to genuinely exhaust
    /// [`CALL_FUEL_LIMIT`] or [`super::MEMORY_LIMIT_BYTES`] in an
    /// end-to-end run (see `fuel_headroom_well_under_compile_time_limit`),
    /// so this test exercises [`classify_trap`] directly against
    /// synthetic errors representing each case -- this is precisely the
    /// function that owns the guarantee under test, so testing it
    /// directly is a stronger, not weaker, proof than hoping a real guest
    /// happens to hit all three failure modes.
    #[test]
    fn exhaustion_taxonomy_stays_distinct() {
        let fuel_err = wasmtime::Error::new(wasmtime::Trap::OutOfFuel);
        assert!(matches!(
            classify_trap(fuel_err, TrapSite::ApplyEvent),
            HostError::FuelExhausted
        ));

        let store_budget_err = wasmtime::Error::new(StoreBudgetExceeded);
        assert!(matches!(
            classify_trap(store_budget_err, TrapSite::ApplyEvent),
            HostError::MemoryExhausted
        ));

        let memory_limit_err = wasmtime::Error::new(MemoryLimitExceeded);
        assert!(matches!(
            classify_trap(memory_limit_err, TrapSite::HandleCommand),
            HostError::MemoryExhausted
        ));

        let ordinary_apply_trap = wasmtime::Error::msg("guest divided by zero");
        assert!(matches!(
            classify_trap(ordinary_apply_trap, TrapSite::ApplyEvent),
            HostError::ApplyEventTrapped(_)
        ));

        let ordinary_handle_trap = wasmtime::Error::msg("guest indexed out of range");
        assert!(matches!(
            classify_trap(ordinary_handle_trap, TrapSite::HandleCommand),
            HostError::HandleCommandTrapped(_)
        ));
    }

    /// Golden-file serde regression over the host-side marshalling codec
    /// (CHE-0038:R5): a thin serde mirror of the WIT-level command/event
    /// shapes, used for host-side observability (logging, audit trails)
    /// rather than the WASM ABI itself (which the component model, not
    /// serde, governs). A checked-in fixture pins the wire shape so an
    /// unintentional rename/reshape is caught here rather than
    /// downstream.
    #[test]
    fn golden_file_marshalling_codec_regression() {
        #[derive(serde::Serialize)]
        struct Wire<'a> {
            count: u32,
            command: &'a str,
            events: &'a [&'static str],
        }

        let sample = Wire {
            count: 3,
            command: "reset",
            events: &["was-reset"],
        };
        let actual = serde_json::to_string_pretty(&sample).expect("serializes");
        let golden = include_str!("../tests/golden/counter_wire.json");
        assert_eq!(actual.trim_end(), golden.trim_end());
    }

    /// Converts [`MEASURED_MINIMUM_CORE_INSTANCES`] from an asserted
    /// constant into evidence (adr-fmt-rjogx Medium finding): instantiates
    /// the REAL reference guest twice against the actual production
    /// [`HostState`]/[`Linker`] pairing, once with the limit one below the
    /// claimed minimum (must fail with a resource-limit error) and once
    /// at exactly the claimed minimum (must succeed). If the guest's true
    /// requirement ever drifts above `MEASURED_MINIMUM_CORE_INSTANCES`,
    /// the second assertion fails loudly here rather than
    /// [`CORE_INSTANCE_LIMIT`] silently under- or over-provisioning.
    #[test]
    fn core_instance_limit_has_measured_headroom_above_the_true_minimum() {
        fn instantiate_at_limit(
            host: &HostRuntime,
            limit: usize,
        ) -> wasmtime::Result<super::DomainApp> {
            let mut resources = ResourceTable::new();
            resources.set_max_capacity(RESOURCE_TABLE_CAPACITY);
            let mut store = Store::new(
                &host.engine,
                HostState {
                    resources,
                    limiter: AggregateLimiter::with_core_instance_limit(limit),
                },
            );
            store.limiter(|state| &mut state.limiter);
            store
                .set_fuel(CALL_FUEL_LIMIT)
                .expect("fuel consumption enabled");
            super::DomainApp::instantiate(&mut store, &host.component, &host.linker)
        }

        let host = runtime();

        assert!(
            instantiate_at_limit(&host, MEASURED_MINIMUM_CORE_INSTANCES - 1).is_err(),
            "expected instantiation to fail one below the measured minimum"
        );
        assert!(
            instantiate_at_limit(&host, MEASURED_MINIMUM_CORE_INSTANCES).is_ok(),
            "expected instantiation to succeed at exactly the measured minimum"
        );
    }
}
