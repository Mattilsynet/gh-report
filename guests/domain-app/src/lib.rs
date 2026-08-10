wit_bindgen::generate!({
    world: "domain-app",
    path: "../../crates/domain-app-host/wit/domain-app",
    additional_derives: [PartialEq],
});

struct Component;

impl Guest for Component {
    fn handle_command(
        state: CounterState,
        cmd: CounterCommand,
    ) -> Result<Vec<CounterEvent>, HandleError> {
        handle_command_logic(state, cmd)
    }

    fn apply_event(state: CounterState, event: CounterEvent) -> CounterState {
        apply_event_logic(state, event)
    }
}

fn handle_command_logic(
    state: CounterState,
    cmd: CounterCommand,
) -> Result<Vec<CounterEvent>, HandleError> {
    match cmd {
        CounterCommand::Increment => Ok(vec![CounterEvent::Incremented]),
        CounterCommand::Reset if state.count == 0 => Err(HandleError::InvariantViolated(
            "already zero".to_string(),
        )),
        CounterCommand::Reset => Ok(vec![CounterEvent::WasReset]),
    }
}

fn apply_event_logic(state: CounterState, event: CounterEvent) -> CounterState {
    match event {
        CounterEvent::Incremented => CounterState {
            count: state.count + 1,
        },
        CounterEvent::WasReset => CounterState { count: 0 },
    }
}

export!(Component);

#[cfg(test)]
mod tests {
    use super::{
        CounterCommand, CounterEvent, CounterState, HandleError, apply_event_logic,
        handle_command_logic,
    };

    #[test]
    fn increment_emits_incremented_event() {
        let state = CounterState { count: 0 };
        let events = handle_command_logic(state, CounterCommand::Increment).expect("ok");
        assert_eq!(events, vec![CounterEvent::Incremented]);
    }

    #[test]
    fn reset_at_zero_is_rejected() {
        let state = CounterState { count: 0 };
        let err = handle_command_logic(state, CounterCommand::Reset).expect_err("err");
        assert_eq!(err, HandleError::InvariantViolated("already zero".to_string()));
    }

    #[test]
    fn reset_above_zero_emits_was_reset_event() {
        let state = CounterState { count: 3 };
        let events = handle_command_logic(state, CounterCommand::Reset).expect("ok");
        assert_eq!(events, vec![CounterEvent::WasReset]);
    }

    #[test]
    fn apply_incremented_increases_count() {
        let state = CounterState { count: 1 };
        let next = apply_event_logic(state, CounterEvent::Incremented);
        assert_eq!(next, CounterState { count: 2 });
    }

    #[test]
    fn apply_was_reset_zeroes_count() {
        let state = CounterState { count: 5 };
        let next = apply_event_logic(state, CounterEvent::WasReset);
        assert_eq!(next, CounterState { count: 0 });
    }
}
