//! Runtime configuration derived from CLI arguments and environment.

use std::path::PathBuf;

use tracing::info;

use crate::config;
use crate::config::dashboard::DashboardConfig;
use crate::error::ConfigError;

/// Runtime configuration for a collection run.
#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    /// Target organization name.
    pub org_name: String,
    /// Disable checkpoint resume.
    pub no_resume: bool,
    /// Maximum concurrent workers for repository checks.
    pub max_workers: usize,
    /// Persistent store directory for baseline, checkpoints, and lock files.
    pub store_dir: PathBuf,
    /// Pardosa authoritative backend selected at startup.
    pub pardosa_backend: PardosaBackend,
    /// Forcibly remove an existing lock before acquiring.
    pub force_unlock: bool,
    /// Dashboard rendering configuration.
    pub dashboard_config: DashboardConfig,
}

/// Pardosa authoritative backend selected once at startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PardosaBackend {
    #[default]
    Pgno,
    Nats,
}

impl RuntimeConfig {
    /// Create a new `RuntimeConfig` with validation.
    ///
    /// `max_workers` is clamped to [`config::MIN_WORKERS`]..=[`config::MAX_WORKERS`].
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::MissingField`] if `org_name` is empty.
    pub fn new(
        org_name: &str,
        no_resume: bool,
        max_workers: usize,
        store_dir: PathBuf,
    ) -> Result<Self, ConfigError> {
        if org_name.trim().is_empty() {
            return Err(ConfigError::MissingField {
                field: "org_name".to_string(),
            });
        }
        let clamped = max_workers.clamp(config::MIN_WORKERS, config::MAX_WORKERS);
        if clamped != max_workers {
            info!(
                requested = max_workers,
                actual = clamped,
                min = config::MIN_WORKERS,
                max = config::MAX_WORKERS,
                "max_workers clamped to allowed range"
            );
        }
        Ok(Self {
            org_name: org_name.trim().to_string(),
            no_resume,
            max_workers: clamped,
            store_dir,
            pardosa_backend: PardosaBackend::Pgno,
            force_unlock: false,
            dashboard_config: DashboardConfig::default(),
        })
    }

    /// Create a new `RuntimeConfig` with `force_unlock` set.
    ///
    /// Same as [`new`](Self::new) but also sets `force_unlock`.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::MissingField`] if `org_name` is empty.
    pub fn with_force_unlock(
        org_name: &str,
        no_resume: bool,
        max_workers: usize,
        store_dir: PathBuf,
        force_unlock: bool,
        dashboard_config: DashboardConfig,
    ) -> Result<Self, ConfigError> {
        let mut config = Self::new(org_name, no_resume, max_workers, store_dir)?;
        config.force_unlock = force_unlock;
        config.dashboard_config = dashboard_config;
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_config_valid() {
        let cfg = RuntimeConfig::new("my-org", false, 8, PathBuf::from("store"));
        assert!(cfg.is_ok());
        let cfg = cfg.unwrap();
        assert_eq!(cfg.org_name, "my-org");
        assert_eq!(cfg.max_workers, 8);
        assert_eq!(cfg.pardosa_backend, PardosaBackend::Pgno);
    }

    #[test]
    fn runtime_config_clamps_workers() {
        let cfg = RuntimeConfig::new("org", false, 0, PathBuf::from("s")).unwrap();
        assert_eq!(cfg.max_workers, config::MIN_WORKERS);

        let cfg = RuntimeConfig::new("org", false, 9999, PathBuf::from("s")).unwrap();
        assert_eq!(cfg.max_workers, config::MAX_WORKERS);
    }

    #[test]
    fn runtime_config_rejects_empty_org() {
        let result = RuntimeConfig::new("", false, 8, PathBuf::from("s"));
        assert!(matches!(result, Err(ConfigError::MissingField { .. })));
    }

    #[test]
    fn runtime_config_rejects_whitespace_org() {
        let result = RuntimeConfig::new("  ", false, 8, PathBuf::from("s"));
        assert!(matches!(result, Err(ConfigError::MissingField { .. })));
    }

    #[test]
    fn runtime_config_trims_org_name() {
        let cfg = RuntimeConfig::new("  my-org  ", false, 8, PathBuf::from("s")).unwrap();
        assert_eq!(cfg.org_name, "my-org");
    }

    #[test]
    fn runtime_config_selects_nats_backend() {
        let mut cfg = RuntimeConfig::new("org", false, 8, PathBuf::from("s")).unwrap();
        cfg.pardosa_backend = PardosaBackend::Nats;
        assert_eq!(cfg.pardosa_backend, PardosaBackend::Nats);
    }
}
