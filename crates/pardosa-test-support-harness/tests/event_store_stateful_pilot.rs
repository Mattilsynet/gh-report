//! Bounded stateful facade pilot (ADR-0018 §D1/§D2/§D4/§D5,
//! ADR-0011 §D2/§D5/§D8).
//!
//! Exercises sequences of `pardosa::store` verbs through the public
//! surface only and cross-checks observed reader / cursor state
//! against a minimal in-test model after every step.
//!
//! Operation alphabet (facade verbs only): `Begin`, `AppendLast`,
//! `Sync`, `ReopenTail`, `CommitConsumed`. Uses [`proptest`]
//! state-machine style with bounded length for determinism.
//!
//! Per-step invariants: `acked_lsn` monotonic non-decreasing;
//! `ReopenTail` yields exactly the synced prefix in commit order;
//! `commit_consumed` advances `acked_offset` monotonically and
//! never regresses on a stale id.
use pardosa::store::{Event, EventId, EventStore, GenomeSafe, HasEventSchemaSource, LiveFiber};
use proptest::collection::vec;
use proptest::prelude::*;
use std::path::PathBuf;
use tempfile::TempDir;
#[derive(Debug, Clone, PartialEq, Eq, GenomeSafe)]
struct Payload {
    v: u64,
}
impl HasEventSchemaSource for Payload {
    const EVENT_SCHEMA_SOURCE: Option<&'static str> = None;
}
#[derive(Debug, Clone)]
enum Op {
    Begin(u64),
    AppendLast(u64),
    Sync,
    ReopenTail,
    CommitConsumed(usize),
}
fn op_strategy() -> impl Strategy<Value = Op> {
    prop_oneof![
        4 => any::< u64 > ().prop_map(Op::Begin), 4 => any::< u64 > ()
        .prop_map(Op::AppendLast), 3 => Just(Op::Sync), 2 => Just(Op::ReopenTail), 1 =>
        (0usize..8).prop_map(Op::CommitConsumed),
    ]
}
#[derive(Default)]
struct Model {
    pending: Vec<EventId>,
    synced: Vec<EventId>,
    acked_lsn: Option<u64>,
    acked_offset: Option<EventId>,
}
struct World {
    td: TempDir,
    journal: PathBuf,
    sidecar: PathBuf,
    store: EventStore<Payload>,
    last_live: Option<LiveFiber>,
}
impl World {
    fn new() -> Self {
        let td = TempDir::new().expect("tempdir");
        let journal = td.path().join("journal.pgno");
        let sidecar = td.path().join("sidecar.ack");
        let store = EventStore::<Payload>::create(&journal).expect("create");
        Self {
            td,
            journal,
            sidecar,
            store,
            last_live: None,
        }
    }
    fn reopen(&mut self) {
        let store = EventStore::<Payload>::open(&self.journal).expect("reopen after drop");
        self.store = store;
        self.last_live = None;
    }
    fn tail_events(&mut self, sidecar: &std::path::Path) -> Vec<Event<Payload>> {
        let reader = self.store.reader();
        let mut cur = reader.cursor(sidecar).expect("cursor");
        cur.tail().map(|r| r.expect("tail item")).collect()
    }
}
fn run_program(ops: Vec<Op>) {
    let mut w = World::new();
    let mut m = Model::default();
    let fresh_sidecar = w.td.path().join("fresh.ack");
    for op in ops {
        match op {
            Op::Begin(v) => {
                let r = w.store.writer().begin(Payload { v }).expect("facade begin");
                m.pending.push(r.event_id());
                w.last_live = Some(r.fiber());
            }
            Op::AppendLast(v) => {
                if let Some(live) = w.last_live.take() {
                    let r = w
                        .store
                        .writer()
                        .append(live, Payload { v })
                        .expect("facade append");
                    m.pending.push(r.event_id());
                    w.last_live = Some(r.fiber());
                }
            }
            Op::Sync => {
                let lsn = w.store.writer().sync().expect("facade sync ok");
                let lsn_u = lsn.value();
                if let Some(prev) = m.acked_lsn {
                    assert!(
                        lsn_u >= prev,
                        "acked_lsn must be monotonic non-decreasing: {prev} -> {lsn_u}"
                    );
                }
                m.acked_lsn = Some(lsn_u);
                m.synced.append(&mut m.pending);
            }
            Op::ReopenTail => {
                if m.acked_lsn.is_none() {
                    continue;
                }
                w.reopen();
                let evs = w.tail_events(&fresh_sidecar);
                let ids: Vec<EventId> = evs.iter().map(pardosa::store::Event::event_id).collect();
                assert_eq!(
                    ids, m.synced,
                    "fresh-sidecar reopen tail must equal the synced prefix"
                );
                m.pending.clear();
            }
            Op::CommitConsumed(k) => {
                if m.acked_lsn.is_none() {
                    continue;
                }
                let sidecar_path = w.sidecar.clone();
                let evs = w.tail_events(&sidecar_path);
                if evs.is_empty() || k >= evs.len() {
                    continue;
                }
                let target = &evs[k];
                let target_id = target.event_id();
                {
                    let reader = w.store.reader();
                    let mut cur = reader.cursor(&sidecar_path).expect("cursor");
                    cur.commit_consumed(target).expect("commit_consumed ok");
                    let observed = cur.acked_offset();
                    let expected = match m.acked_offset {
                        Some(prev) if prev.value() >= target_id.value() => Some(prev),
                        _ => Some(target_id),
                    };
                    assert_eq!(
                        observed, expected,
                        "commit_consumed monotonic + stale-noop semantics broken"
                    );
                    m.acked_offset = observed;
                }
            }
        }
    }
}
proptest! {
    #![proptest_config(ProptestConfig { cases : 24, max_shrink_iters : 64,
    ..ProptestConfig::default() })] #[test] fn
    stateful_facade_pilot_preserves_invariants(ops in vec(op_strategy(), 0..18)) {
    run_program(ops); }
}
#[test]
fn stateful_facade_pilot_deterministic_smoke_sequence() {
    run_program(vec![
        Op::Begin(1),
        Op::AppendLast(2),
        Op::Sync,
        Op::Begin(3),
        Op::AppendLast(4),
        Op::ReopenTail,
        Op::Begin(5),
        Op::Sync,
        Op::CommitConsumed(0),
        Op::CommitConsumed(0),
    ]);
}
