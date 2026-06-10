//! Repo merger arm for the single durable gh-report event stream.

use cherry_pit_core::{Aggregate, HandleCommand};
use cherry_pit_merger::{MergerArm, PersistMode};

use crate::domain::aggregates::repo::{RecordEvaluation, RecordRemoval, Repo, RepoError};

/// Channel payload for the [`Repo`](crate::domain::aggregates::repo::Repo)
/// merger arm.
#[derive(Debug)]
pub enum RepoCmd {
    /// Repository state was captured.
    Evaluate {
        domain_key: String,
        cmd: Box<RecordEvaluation>,
    },
    /// Repository tombstone was captured.
    Remove {
        domain_key: String,
        cmd: RecordRemoval,
    },
}

/// [`MergerArm`] impl for the Repo aggregate.
#[derive(Debug, Default)]
pub struct RepoArm;

impl MergerArm<Repo> for RepoArm {
    type Cmd = RepoCmd;
    type Err = RepoError;

    fn persist_mode(&self, cmd: &Self::Cmd) -> PersistMode {
        match cmd {
            RepoCmd::Evaluate { domain_key, .. } | RepoCmd::Remove { domain_key, .. } => {
                PersistMode::CreateOrAppend(domain_key.clone())
            }
        }
    }

    fn handle(
        &self,
        state: &Repo,
        cmd: Self::Cmd,
    ) -> Result<Vec<<Repo as Aggregate>::Event>, Self::Err> {
        match cmd {
            RepoCmd::Evaluate { cmd, .. } => state.handle(*cmd),
            RepoCmd::Remove { cmd, .. } => state.handle(cmd),
        }
    }

    fn publish_label(&self, cmd: &Self::Cmd) -> &'static str {
        match cmd {
            RepoCmd::Evaluate { .. } | RepoCmd::Remove { .. } => "RepositoryStateCaptured",
        }
    }
}
