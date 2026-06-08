use crate::error::PardosaError;
use serde::{Deserialize, Serialize};
/// Lifecycle state for a fiber under ADR-0003 ("Fiber semantics").
///
/// See the rendered state diagram at
/// [`docs/fiber_state_machine.svg`](https://github.com/acje/rescue-pardosa/blob/main/docs/fiber_state_machine.svg)
/// (mission rescue-pardosa-fhxn doc lift).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum FiberState {
    Undefined,
    Defined,
    Detached,
    Purged,
    Locked,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum FiberMigrationPolicy {
    Keep,
    Purge,
    LockAndPrune,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum LockedRescuePolicy {
    PreserveAuditTrail,
    AcceptDataLoss,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum FiberAction {
    Create,
    Update,
    Detach,
    Rescue,
    Migrate(FiberMigrationPolicy),
}
pub(crate) const TRANSITIONS: &[(FiberState, FiberAction, FiberState)] = &[
    (
        FiberState::Undefined,
        FiberAction::Create,
        FiberState::Defined,
    ),
    (
        FiberState::Defined,
        FiberAction::Update,
        FiberState::Defined,
    ),
    (
        FiberState::Defined,
        FiberAction::Detach,
        FiberState::Detached,
    ),
    (
        FiberState::Detached,
        FiberAction::Rescue,
        FiberState::Defined,
    ),
    (
        FiberState::Detached,
        FiberAction::Migrate(FiberMigrationPolicy::Keep),
        FiberState::Detached,
    ),
    (
        FiberState::Detached,
        FiberAction::Migrate(FiberMigrationPolicy::Purge),
        FiberState::Purged,
    ),
    (
        FiberState::Detached,
        FiberAction::Migrate(FiberMigrationPolicy::LockAndPrune),
        FiberState::Locked,
    ),
    (FiberState::Purged, FiberAction::Create, FiberState::Defined),
    (FiberState::Locked, FiberAction::Rescue, FiberState::Defined),
    (
        FiberState::Locked,
        FiberAction::Migrate(FiberMigrationPolicy::Purge),
        FiberState::Purged,
    ),
];
/// Look up the target state for `(state, action)` in the transition table.
///
/// # Errors
/// Returns `PardosaError::InvalidTransition { state, action }` when no
/// transition row matches the pair.
pub(crate) fn transition(
    state: FiberState,
    action: FiberAction,
) -> Result<FiberState, PardosaError> {
    TRANSITIONS
        .iter()
        .find(|(s, a, _)| *s == state && *a == action)
        .map(|(_, _, target)| *target)
        .ok_or(PardosaError::InvalidTransition { state, action })
}
#[cfg(test)]
mod tests {
    use super::*;
    const ALL_STATES: [FiberState; 5] = [
        FiberState::Undefined,
        FiberState::Defined,
        FiberState::Detached,
        FiberState::Purged,
        FiberState::Locked,
    ];
    const ALL_ACTIONS: [FiberAction; 7] = [
        FiberAction::Create,
        FiberAction::Update,
        FiberAction::Detach,
        FiberAction::Rescue,
        FiberAction::Migrate(FiberMigrationPolicy::Keep),
        FiberAction::Migrate(FiberMigrationPolicy::Purge),
        FiberAction::Migrate(FiberMigrationPolicy::LockAndPrune),
    ];
    #[test]
    fn valid_transition_count() {
        assert_eq!(TRANSITIONS.len(), 10);
    }
    #[test]
    fn exhaustive_35_pairs() {
        let mut valid = 0;
        let mut invalid = 0;
        for state in &ALL_STATES {
            for action in &ALL_ACTIONS {
                match transition(*state, *action) {
                    Ok(_) => valid += 1,
                    Err(_) => invalid += 1,
                }
            }
        }
        assert_eq!(valid, 10);
        assert_eq!(invalid, 25);
    }
    #[test]
    fn undefined_create_defined() {
        assert_eq!(
            transition(FiberState::Undefined, FiberAction::Create).unwrap(),
            FiberState::Defined
        );
    }
    #[test]
    fn defined_update_defined() {
        assert_eq!(
            transition(FiberState::Defined, FiberAction::Update).unwrap(),
            FiberState::Defined
        );
    }
    #[test]
    fn defined_detach_detached() {
        assert_eq!(
            transition(FiberState::Defined, FiberAction::Detach).unwrap(),
            FiberState::Detached
        );
    }
    #[test]
    fn detached_rescue_defined() {
        assert_eq!(
            transition(FiberState::Detached, FiberAction::Rescue).unwrap(),
            FiberState::Defined
        );
    }
    #[test]
    fn detached_migrate_keep_detached() {
        assert_eq!(
            transition(
                FiberState::Detached,
                FiberAction::Migrate(FiberMigrationPolicy::Keep)
            )
            .unwrap(),
            FiberState::Detached
        );
    }
    #[test]
    fn detached_migrate_purge_purged() {
        assert_eq!(
            transition(
                FiberState::Detached,
                FiberAction::Migrate(FiberMigrationPolicy::Purge)
            )
            .unwrap(),
            FiberState::Purged
        );
    }
    #[test]
    fn detached_migrate_lockandprune_locked() {
        assert_eq!(
            transition(
                FiberState::Detached,
                FiberAction::Migrate(FiberMigrationPolicy::LockAndPrune)
            )
            .unwrap(),
            FiberState::Locked
        );
    }
    #[test]
    fn purged_create_defined() {
        assert_eq!(
            transition(FiberState::Purged, FiberAction::Create).unwrap(),
            FiberState::Defined
        );
    }
    #[test]
    fn locked_rescue_defined() {
        assert_eq!(
            transition(FiberState::Locked, FiberAction::Rescue).unwrap(),
            FiberState::Defined
        );
    }
    #[test]
    fn locked_migrate_purge_purged() {
        assert_eq!(
            transition(
                FiberState::Locked,
                FiberAction::Migrate(FiberMigrationPolicy::Purge)
            )
            .unwrap(),
            FiberState::Purged
        );
    }
    #[test]
    fn undefined_update_invalid() {
        assert!(transition(FiberState::Undefined, FiberAction::Update).is_err());
    }
    #[test]
    fn defined_create_invalid() {
        assert!(transition(FiberState::Defined, FiberAction::Create).is_err());
    }
    #[test]
    fn purged_rescue_invalid() {
        assert!(transition(FiberState::Purged, FiberAction::Rescue).is_err());
    }
    #[test]
    fn locked_create_invalid() {
        assert!(transition(FiberState::Locked, FiberAction::Create).is_err());
    }
    #[test]
    fn no_duplicate_state_action_pairs() {
        use std::collections::HashSet;
        let mut seen = HashSet::new();
        for (s, a, _) in TRANSITIONS {
            assert!(
                seen.insert((*s, *a)),
                "duplicate transition: ({s:?}, {a:?})"
            );
        }
    }
}
