//! Domain definitions for the 2-aggregate fixture.
//!
//! Two aggregates `Foo` and `Bar`, one command + one event each, one
//! cross-aggregate policy `FooToBarPolicy` whose output enum drives
//! a `BarPing` command into the `Bar` aggregate.

use std::convert::Infallible;

use cherry_pit_core::{Aggregate, Command, DomainEvent, EventEnvelope, HandleCommand, Policy};
use serde::{Deserialize, Serialize};

// ── Foo aggregate ────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FooEvent {
    Happened { value: u32 },
}

impl DomainEvent for FooEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::Happened { .. } => "foo.happened",
        }
    }
}

impl pardosa_encoding::Encode for FooEvent {
    fn encode(&self, out: &mut Vec<u8>) {
        match self {
            Self::Happened { value } => {
                out.push(0u8);
                pardosa_encoding::Encode::encode(value, out);
            }
        }
    }
}

#[derive(Debug, Default)]
pub struct Foo {
    pub last: u32,
}

impl Aggregate for Foo {
    type Event = FooEvent;
    fn apply(&mut self, e: &FooEvent) {
        match e {
            FooEvent::Happened { value } => self.last = *value,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct FooDo {
    pub value: u32,
}

impl Command for FooDo {}

impl HandleCommand<FooDo> for Foo {
    type Error = Infallible;
    fn handle(&self, cmd: FooDo) -> Result<Vec<FooEvent>, Self::Error> {
        Ok(vec![FooEvent::Happened { value: cmd.value }])
    }
}

// ── Bar aggregate ────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum BarEvent {
    Pinged { from: u32 },
}

impl DomainEvent for BarEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::Pinged { .. } => "bar.pinged",
        }
    }
}

impl pardosa_encoding::Encode for BarEvent {
    fn encode(&self, out: &mut Vec<u8>) {
        match self {
            Self::Pinged { from } => {
                out.push(0u8);
                pardosa_encoding::Encode::encode(from, out);
            }
        }
    }
}

#[derive(Debug, Default)]
pub struct Bar {
    pub pings: u32,
}

impl Aggregate for Bar {
    type Event = BarEvent;
    fn apply(&mut self, e: &BarEvent) {
        match e {
            BarEvent::Pinged { .. } => self.pings += 1,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct BarPing {
    pub from: u32,
}

impl Command for BarPing {}

impl HandleCommand<BarPing> for Bar {
    type Error = Infallible;
    fn handle(&self, cmd: BarPing) -> Result<Vec<BarEvent>, Self::Error> {
        Ok(vec![BarEvent::Pinged { from: cmd.from }])
    }
}

// ── Cross-aggregate policy: Foo events drive Bar pings ──────────────

#[derive(Debug, PartialEq, Eq)]
pub enum FooToBarOutput {
    Ping(BarPing),
}

pub struct FooToBarPolicy;

impl Policy for FooToBarPolicy {
    type Event = FooEvent;
    type Output = FooToBarOutput;
    fn react(&self, env: &EventEnvelope<FooEvent>) -> Vec<FooToBarOutput> {
        match env.payload() {
            FooEvent::Happened { value } => {
                vec![FooToBarOutput::Ping(BarPing { from: *value })]
            }
        }
    }
}
