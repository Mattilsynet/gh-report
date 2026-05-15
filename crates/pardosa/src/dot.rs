use std::fmt::Write as _;

use crate::fiber_state::{FiberAction, FiberState, MigrationPolicy, TRANSITIONS};

/// Generate DOT/Graphviz representation of the fiber state machine
/// from the TRANSITIONS table.
#[must_use]
pub fn transitions_to_dot() -> String {
    let mut dot =
        String::from("digraph FiberStateMachine {\n    rankdir=LR;\n    node [shape=ellipse];\n\n");

    for (from, action, to) in TRANSITIONS {
        let label = action_label(*action);
        let _ = writeln!(
            dot,
            "    {} -> {} [label=\"{label}\"];",
            state_name(*from),
            state_name(*to),
        );
    }

    dot.push_str("}\n");
    dot
}

fn state_name(s: FiberState) -> &'static str {
    match s {
        FiberState::Undefined => "Undefined",
        FiberState::Defined => "Defined",
        FiberState::Detached => "Detached",
        FiberState::Purged => "Purged",
        FiberState::Locked => "Locked",
    }
}

fn action_label(a: FiberAction) -> String {
    match a {
        FiberAction::Create => "Create".into(),
        FiberAction::Update => "Update".into(),
        FiberAction::Detach => "Detach".into(),
        FiberAction::Rescue => "Rescue".into(),
        FiberAction::Migrate(policy) => {
            let p = match policy {
                MigrationPolicy::Keep => "Keep",
                MigrationPolicy::Purge => "Purge",
                MigrationPolicy::LockAndPrune => "LockAndPrune",
            };
            format!("Migrate({p})")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dot_contains_all_transitions() {
        let dot = transitions_to_dot();
        assert!(dot.contains("Undefined -> Defined"));
        assert!(dot.contains("Defined -> Defined"));
        assert!(dot.contains("Defined -> Detached"));
        assert!(dot.contains("Detached -> Defined"));
        assert!(dot.contains("Detached -> Detached"));
        assert!(dot.contains("Detached -> Purged"));
        assert!(dot.contains("Detached -> Locked"));
        assert!(dot.contains("Purged -> Defined"));
        assert!(dot.contains("Locked -> Defined"));
        assert!(dot.contains("Locked -> Purged"));
        // Verify all 10 edges present
        assert_eq!(dot.matches("->").count(), 10);
    }
}
