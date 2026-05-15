# PAR-0017. State Machine Bus Architecture

Date: 2026-04-29
Last-reviewed: 2026-04-29
Tier: S
Status: Accepted

## Related

References: PAR-0008, GND-0005

## Context

Pardosa's fiber lifecycle is already shaped as a synchronous state machine
(PAR-0001), but the surrounding components — publisher, registry watcher,
migration driver, genome encoder — currently mix `async fn` calls with
state mutation. This makes deterministic simulation testing impossible:
async scheduling, network I/O, and entropy sources all leak
non-determinism into core invariants.

Polar Signals (Jul 2025) and S2.dev (Apr 2025) document the same answer:
factor every component into a synchronous `StateMachine` exchanging
messages on a single bus. The bus is the only scheduler, drives both real
runtime and simulation, and makes failure injection (drop, delay,
reorder) a property of the bus rather than per-component fault hooks.

## Decision

Every pardosa component that owns mutable state is a `StateMachine` with
a non-async `receive(Msg) -> Vec<(Msg, Destination)>` and `tick(Instant)
-> Vec<(Msg, Destination)>`. All inter-component communication flows
through a single `MessageBus`. Async I/O is performed only by edge
drivers that translate bus messages to and from the outside world.

R1 [2]: Model every stateful pardosa component as a synchronous
  StateMachine trait with non-async receive and tick methods returning
  outbound messages
R2 [4]: Route every inter-component message through a single MessageBus
  abstraction so the bus is the only scheduler in production and
  simulation
R3 [5]: Confine async operations to edge drivers that translate bus
  messages to and from external systems like NATS, the filesystem, and
  the host clock
R4 [5]: Expose every public pardosa entry point as a bus dispatch so no
  caller path bypasses the StateMachine layer

## Consequences

Determinism becomes a structural property — replacing the bus driver
swaps real execution for simulation without touching component code.
Failure injection lives in one place. Trade-offs: the refactor touches
the entire crate and forbids ergonomic `async fn` methods on stateful
types. Code that "should be in the state machine" tends to leak into
drivers (Polar Signals' caution); enforcement is via R4 and a periodic
audit. State-machine boundaries become an architectural commitment that
later phases (concurrency, NATS publish, migration) must respect.
