use stateright::{Model, Property};

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
enum Seq {
    Zero,
    One,
    Two,
    Three,
}

impl Seq {
    fn next(self) -> Option<Self> {
        match self {
            Self::Zero => Some(Self::One),
            Self::One => Some(Self::Two),
            Self::Two => Some(Self::Three),
            Self::Three => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Run {
    Initial,
    Recovery,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Receipt {
    Committed(Seq),
    Conflict,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Stop {
    Crash,
    LostAck,
    Conflict,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Phase {
    Ready,
    Sent(Seq),
    CrashedSent(Seq),
    Awaiting(Receipt),
    CrashedAwaiting(Receipt),
    Stopped(Stop),
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct Writer {
    run: Run,
    phase: Phase,
    local: Seq,
    issued: Seq,
    active: Option<usize>,
    pending_updates: Vec<(usize, Seq)>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct Request {
    writer: usize,
    run: Run,
    expected: Seq,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct Record {
    request: usize,
    committed: Seq,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct State {
    writers: Vec<Writer>,
    log: Vec<Record>,
    requests: Vec<Request>,
}

impl State {
    fn tip(&self) -> Seq {
        self.log.last().map_or(Seq::Zero, |event| event.committed)
    }

    fn cas_history(&self) -> bool {
        self.log
            .iter()
            .try_fold(Seq::Zero, |previous, event| {
                let request = self.requests.get(event.request)?;
                (request.expected == previous && previous.next() == Some(event.committed))
                    .then_some(event.committed)
            })
            .is_some()
    }

    fn unique_requests(&self) -> bool {
        self.log.iter().enumerate().all(|(index, event)| {
            !self.log[..index]
                .iter()
                .any(|prior| prior.request == event.request)
        })
    }

    fn receipts_have_commits(&self) -> bool {
        self.writers.iter().all(|writer| {
            let backed = |request, seq| {
                self.log
                    .iter()
                    .any(|event| Some(event.request) == request && event.committed == seq)
            };
            let receipt_backed = match writer.phase {
                Phase::Awaiting(Receipt::Committed(seq))
                | Phase::CrashedAwaiting(Receipt::Committed(seq)) => backed(writer.active, seq),
                _ => true,
            };
            receipt_backed
                && writer
                    .pending_updates
                    .iter()
                    .all(|&(request, seq)| backed(Some(request), seq))
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Population {
    Two,
    Three,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Step {
    Send,
    Broker,
    Deliver,
    Update,
    LoseAck,
    DiscardCrashedReceipt,
    Crash,
    ReplayNewRun,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Action(usize, Step);

#[derive(Clone)]
struct FenceModel(Population, AppendMode);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AppendMode {
    CompareAndSwap,
    BlindAppend,
}

impl Model for FenceModel {
    type State = State;
    type Action = Action;

    fn init_states(&self) -> Vec<State> {
        let count = match self.0 {
            Population::Two => 2,
            Population::Three => 3,
        };
        vec![State {
            writers: vec![
                Writer {
                    run: Run::Initial,
                    phase: Phase::Ready,
                    local: Seq::Zero,
                    issued: Seq::Zero,
                    active: None,
                    pending_updates: Vec::new(),
                };
                count
            ],
            log: Vec::new(),
            requests: Vec::new(),
        }]
    }

    fn actions(&self, state: &State, actions: &mut Vec<Action>) {
        for id in 0..state.writers.len() {
            for step in [
                Step::Send,
                Step::Broker,
                Step::Deliver,
                Step::Update,
                Step::LoseAck,
                Step::DiscardCrashedReceipt,
                Step::Crash,
                Step::ReplayNewRun,
            ] {
                let action = Action(id, step);
                if self.next_state(state, action).is_some() {
                    actions.push(action);
                }
            }
        }
    }

    fn next_state(&self, state: &State, Action(id, step): Action) -> Option<State> {
        let writer = state.writers.get(id)?;
        let mut next = state.clone();
        let phase = match (writer.phase, step) {
            (Phase::Ready, Step::Send) if writer.local.next().is_some() => {
                next.writers[id].issued = writer.issued.next()?;
                next.writers[id].active = Some(state.requests.len());
                next.requests.push(Request {
                    writer: id,
                    run: writer.run,
                    expected: writer.local,
                });
                Phase::Sent(writer.local)
            }
            (Phase::Sent(expected) | Phase::CrashedSent(expected), Step::Broker) => {
                let receipt = if self.1 == AppendMode::BlindAppend || expected == state.tip() {
                    let committed = state.tip().next()?;
                    next.log.push(Record {
                        request: writer.active?,
                        committed,
                    });
                    Receipt::Committed(committed)
                } else {
                    Receipt::Conflict
                };
                match writer.phase {
                    Phase::CrashedSent(_) => Phase::CrashedAwaiting(receipt),
                    _ => Phase::Awaiting(receipt),
                }
            }
            (Phase::Awaiting(Receipt::Committed(seq)), Step::Deliver) => {
                next.writers[id].pending_updates.push((writer.active?, seq));
                next.writers[id].active = None;
                Phase::Ready
            }
            (Phase::Awaiting(Receipt::Conflict), Step::Deliver) => Phase::Stopped(Stop::Conflict),
            (phase, Step::Update) if !writer.pending_updates.is_empty() => {
                let (_, seq) = next.writers[id].pending_updates.remove(0);
                next.writers[id].local = writer.local.max(seq);
                phase
            }
            (Phase::Awaiting(_), Step::LoseAck) => Phase::Stopped(Stop::LostAck),
            (Phase::CrashedAwaiting(_), Step::DiscardCrashedReceipt) => Phase::Stopped(Stop::Crash),
            (Phase::Sent(expected), Step::Crash) => Phase::CrashedSent(expected),
            (Phase::Awaiting(receipt), Step::Crash) => Phase::CrashedAwaiting(receipt),
            (Phase::Ready, Step::Crash) => Phase::Stopped(Stop::Crash),
            (Phase::Stopped(_), Step::ReplayNewRun)
                if writer.run == Run::Initial && writer.pending_updates.is_empty() =>
            {
                next.writers[id].run = Run::Recovery;
                next.writers[id].local = state.tip();
                next.writers[id].issued = Seq::Zero;
                next.writers[id].active = None;
                Phase::Ready
            }
            _ => return None,
        };
        if matches!(
            phase,
            Phase::Stopped(Stop::Crash) | Phase::CrashedSent(_) | Phase::CrashedAwaiting(_)
        ) {
            next.writers[id].pending_updates.clear();
        }
        next.writers[id].phase = phase;
        Some(next)
    }

    fn properties(&self) -> Vec<Property<Self>> {
        vec![
            Property::always("atomic CAS history", |_, s| s.cas_history()),
            Property::always("request appended at most once", |_, s| s.unique_requests()),
            Property::always("receipts name durable commits", |_, s| {
                s.receipts_have_commits()
            }),
            Property::always("sequence capacity", |_, s| s.log.len() <= 3),
            Property::sometimes("capacity reachable", |_, s| s.tip() == Seq::Three),
            Property::sometimes("conflict reachable", |_, s| {
                s.writers
                    .iter()
                    .any(|w| w.phase == Phase::Stopped(Stop::Conflict))
            }),
            Property::sometimes("lost committed ack reachable", |_, s| {
                s.writers.iter().enumerate().any(|(id, w)| {
                    w.phase == Phase::Stopped(Stop::LostAck)
                        && s.log.iter().any(|e| {
                            s.requests[e.request].writer == id && s.requests[e.request].run == w.run
                        })
                })
            }),
            Property::sometimes("recovery commit reachable", |_, s| {
                s.log
                    .iter()
                    .any(|e| s.requests[e.request].run == Run::Recovery)
            }),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stateright::Checker;

    fn advance(model: &FenceModel, state: &mut State, id: usize, step: Step) {
        *state = model
            .next_state(state, Action(id, step))
            .expect("enabled transition");
    }

    #[test]
    fn exhaustive_bounded_safety() {
        for population in [Population::Two, Population::Three] {
            let checker = FenceModel(population, AppendMode::CompareAndSwap)
                .checker()
                .threads(1)
                .spawn_bfs()
                .join();
            assert!(checker.is_done());
            checker.assert_properties();
            eprintln!(
                "population={population:?} states={} depth={}",
                checker.unique_state_count(),
                checker.max_depth()
            );
        }
    }

    #[test]
    fn stale_writer_conflicts_without_append_or_inband_retry() {
        let model = FenceModel(Population::Two, AppendMode::CompareAndSwap);
        let mut state = model.init_states().remove(0);
        for (id, step) in [
            (0, Step::Send),
            (1, Step::Send),
            (0, Step::Broker),
            (1, Step::Broker),
            (1, Step::Deliver),
        ] {
            advance(&model, &mut state, id, step);
        }
        assert_eq!(state.log.len(), 1);
        assert_eq!(state.writers[1].phase, Phase::Stopped(Stop::Conflict));
        assert!(model.next_state(&state, Action(1, Step::Send)).is_none());
        advance(&model, &mut state, 1, Step::ReplayNewRun);
        assert_eq!(
            state.writers[1],
            Writer {
                run: Run::Recovery,
                phase: Phase::Ready,
                local: Seq::One,
                issued: Seq::Zero,
                active: None,
                pending_updates: Vec::new(),
            }
        );
    }

    #[test]
    fn crash_before_send_has_no_append_but_sent_request_can_commit_after_crash() {
        let model = FenceModel(Population::Two, AppendMode::CompareAndSwap);
        let mut state = model.init_states().remove(0);
        advance(&model, &mut state, 0, Step::Crash);
        assert!(state.log.is_empty());
        advance(&model, &mut state, 1, Step::Send);
        advance(&model, &mut state, 1, Step::Crash);
        advance(&model, &mut state, 1, Step::Broker);
        assert_eq!(state.tip(), Seq::One);
        assert_eq!(
            state.writers[1].phase,
            Phase::CrashedAwaiting(Receipt::Committed(Seq::One))
        );
    }

    #[test]
    fn postcommit_crash_and_lost_ack_preserve_history() {
        let model = FenceModel(Population::Two, AppendMode::CompareAndSwap);
        for crash in [false, true] {
            let mut state = model.init_states().remove(0);
            advance(&model, &mut state, 0, Step::Send);
            advance(&model, &mut state, 0, Step::Broker);
            let committed = state.log.clone();
            if crash {
                advance(&model, &mut state, 0, Step::Crash);
            }
            advance(
                &model,
                &mut state,
                0,
                if crash {
                    Step::DiscardCrashedReceipt
                } else {
                    Step::LoseAck
                },
            );
            assert_eq!(state.log, committed);
            assert!(model.next_state(&state, Action(0, Step::Send)).is_none());
        }
    }

    #[test]
    fn same_handle_send_in_release_update_window_is_fenced() {
        let model = FenceModel(Population::Two, AppendMode::CompareAndSwap);
        let mut state = model.init_states().remove(0);
        for step in [Step::Send, Step::Broker, Step::Deliver] {
            advance(&model, &mut state, 0, step);
        }
        assert_eq!(state.writers[0].phase, Phase::Ready);
        assert_eq!(state.writers[0].local, Seq::Zero);
        advance(&model, &mut state, 0, Step::Send);
        advance(&model, &mut state, 0, Step::Update);
        assert_eq!(state.writers[0].local, Seq::One);
        advance(&model, &mut state, 0, Step::Broker);
        advance(&model, &mut state, 0, Step::Deliver);
        assert_eq!(state.writers[0].phase, Phase::Stopped(Stop::Conflict));
        assert_eq!(state.log.len(), 1);
        assert_eq!(state.requests[0], state.requests[1]);
        assert_ne!(state.writers[0].active, Some(state.log[0].request));
    }

    #[test]
    fn invariant_oracles_reject_deliberately_corrupt_states() {
        let model = FenceModel(Population::Two, AppendMode::CompareAndSwap);
        let mut state = model.init_states().remove(0);
        assert!(state.cas_history() && state.unique_requests() && state.receipts_have_commits());
        for step in [Step::Send, Step::Broker] {
            advance(&model, &mut state, 0, step);
        }
        assert!(state.cas_history() && state.unique_requests() && state.receipts_have_commits());
        state.requests[0].expected = Seq::One;
        assert!(!state.cas_history());
        state.log.push(state.log[0].clone());
        assert!(!state.unique_requests());
        state.writers[1].pending_updates.push((0, Seq::Three));
        assert!(!state.receipts_have_commits());
    }

    #[test]
    fn blind_append_has_an_actual_reachable_cas_counterexample() {
        for mode in [AppendMode::CompareAndSwap, AppendMode::BlindAppend] {
            let model = FenceModel(Population::Two, mode);
            let mut state = model.init_states().remove(0);
            for (id, step) in [
                (0, Step::Send),
                (1, Step::Send),
                (0, Step::Broker),
                (1, Step::Broker),
            ] {
                advance(&model, &mut state, id, step);
            }
            assert_eq!(state.cas_history(), mode == AppendMode::CompareAndSwap);
            assert!(state.unique_requests());
            assert!(state.receipts_have_commits());
            if mode == AppendMode::BlindAppend {
                assert_eq!(state.log.len(), 2);
                assert_eq!(state.requests[state.log[1].request].expected, Seq::Zero);
                assert_eq!(state.log[1].committed, Seq::Two);
            }
        }
    }
}
