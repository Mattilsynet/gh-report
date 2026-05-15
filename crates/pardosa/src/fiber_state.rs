use serde::{Deserialize, Serialize};

use crate::error::PardosaError;

/// Lifecycle state of a fiber.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FiberState {
    Undefined,
    Defined,
    Detached,
    Purged,
    Locked,
}

/// Migration deletion policy.
///
/// - `Keep`: Fiber survives migration unchanged, remains soft-deleted.
/// - `Purge`: Fiber removed from line, retained on optional audit trail, key reusable.
/// - `LockAndPrune`: Fiber pruned to last event, removed from line, key not reusable except via Rescue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MigrationPolicy {
    Keep,
    Purge,
    LockAndPrune,
}

/// Policy for rescuing a Locked fiber.
///
/// Replaces the old `acknowledge_data_loss: bool` parameter. Communicates
/// whether the audit trail is preserved (old stream in grace period) or
/// history is destroyed (old stream expired/deleted).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LockedRescuePolicy {
    /// Old events remain in the deprecated stream's grace period.
    /// The audit trail is the deprecated stream itself.
    PreserveAuditTrail,
    /// Old events will be deleted when the deprecated stream expires.
    /// Caller acknowledges permanent data loss after the grace period.
    AcceptDataLoss,
}

/// Action applied to a fiber.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FiberAction {
    Create,
    Update,
    Detach,
    Rescue,
    Migrate(MigrationPolicy),
}

/// The complete transition table. 10 valid transitions.
pub const TRANSITIONS: &[(FiberState, FiberAction, FiberState)] = &[
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
        FiberAction::Migrate(MigrationPolicy::Keep),
        FiberState::Detached,
    ),
    (
        FiberState::Detached,
        FiberAction::Migrate(MigrationPolicy::Purge),
        FiberState::Purged,
    ),
    (
        FiberState::Detached,
        FiberAction::Migrate(MigrationPolicy::LockAndPrune),
        FiberState::Locked,
    ),
    (FiberState::Purged, FiberAction::Create, FiberState::Defined),
    (FiberState::Locked, FiberAction::Rescue, FiberState::Defined),
    (
        FiberState::Locked,
        FiberAction::Migrate(MigrationPolicy::Purge),
        FiberState::Purged,
    ),
];

/// Look up a transition in the table. Returns the resulting state or `InvalidTransition`.
///
/// # Errors
///
/// Returns [`PardosaError::InvalidTransition`] when the `(state, action)`
/// pair does not appear in the transition table.
pub fn transition(state: FiberState, action: FiberAction) -> Result<FiberState, PardosaError> {
    TRANSITIONS
        .iter()
        .find(|(s, a, _)| *s == state && *a == action)
        .map(|(_, _, target)| *target)
        .ok_or(PardosaError::InvalidTransition { state, action })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// All 5 states.
    const ALL_STATES: [FiberState; 5] = [
        FiberState::Undefined,
        FiberState::Defined,
        FiberState::Detached,
        FiberState::Purged,
        FiberState::Locked,
    ];

    /// All 7 actions.
    const ALL_ACTIONS: [FiberAction; 7] = [
        FiberAction::Create,
        FiberAction::Update,
        FiberAction::Detach,
        FiberAction::Rescue,
        FiberAction::Migrate(MigrationPolicy::Keep),
        FiberAction::Migrate(MigrationPolicy::Purge),
        FiberAction::Migrate(MigrationPolicy::LockAndPrune),
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

    // --- 10 valid transitions ---

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
                FiberAction::Migrate(MigrationPolicy::Keep)
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
                FiberAction::Migrate(MigrationPolicy::Purge)
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
                FiberAction::Migrate(MigrationPolicy::LockAndPrune)
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
                FiberAction::Migrate(MigrationPolicy::Purge)
            )
            .unwrap(),
            FiberState::Purged
        );
    }

    // --- Sample invalid transitions ---

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
