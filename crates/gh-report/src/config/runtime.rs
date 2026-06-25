//! Runtime configuration derived from CLI arguments and environment.

use std::path::PathBuf;

use tracing::info;

use crate::config;
use crate::config::dashboard::DashboardConfig;
use crate::error::ConfigError;

/// Default NATS server URL for the pardosa `Nats` backend.
pub const DEFAULT_NATS_URL: &str = "nats://localhost:4222";

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
    /// NATS server URL used when `pardosa_backend` is `Nats`.
    pub nats_url: String,
    /// NATS `.creds` file path used when `pardosa_backend` is `Nats`.
    pub nats_creds: Option<PathBuf>,
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

/// Derived `JetStream` addressing for one gh-report organization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NatsStoreConfig {
    /// NATS server URL used by the `JetStream` client.
    pub nats_url: String,
    /// Per-org `JetStream` stream name.
    pub stream_name: String,
    /// Per-org single subject bound to the stream.
    pub subject: String,
    /// Per-org durable consumer name used during replay.
    pub durable_consumer: String,
    /// NATS `.creds` file path used for authenticated `JetStream` connects.
    pub credentials_path: Option<PathBuf>,
}

impl NatsStoreConfig {
    /// Derive per-org `JetStream` names from the exact UTF-8 org bytes.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::MissingField`] if `org_name` is empty.
    pub fn for_org(org_name: &str, nats_url: impl Into<String>) -> Result<Self, ConfigError> {
        if org_name.is_empty() {
            return Err(ConfigError::MissingField {
                field: "org_name".to_string(),
            });
        }
        let token = org_token(org_name.as_bytes());
        Ok(Self {
            nats_url: nats_url.into(),
            stream_name: format!("gh-report-{token}"),
            subject: format!("gh-report.{token}.events"),
            durable_consumer: format!("gh-report-{token}"),
            credentials_path: None,
        })
    }

    /// Attach a NATS `.creds` file path for authenticated `JetStream` connects.
    #[must_use]
    pub fn with_credentials_path(mut self, credentials_path: Option<PathBuf>) -> Self {
        self.credentials_path = credentials_path;
        self
    }

    /// Derive the distinct org-event `JetStream` names paired with this repo stream.
    #[must_use]
    pub fn org_events(&self) -> Self {
        Self {
            nats_url: self.nats_url.clone(),
            stream_name: format!("{}-org", self.stream_name),
            subject: self.subject.strip_suffix(".events").map_or_else(
                || format!("{}.org.events", self.subject),
                |base| format!("{base}.org.events"),
            ),
            durable_consumer: format!("{}-org", self.durable_consumer),
            credentials_path: self.credentials_path.clone(),
        }
    }
}

fn org_token(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut token = String::with_capacity(4 + bytes.len() * 2);
    token.push_str("org_");
    for byte in bytes {
        token.push(char::from(HEX[usize::from(byte >> 4)]));
        token.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    token
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
            nats_url: DEFAULT_NATS_URL.to_string(),
            nats_creds: None,
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

    /// Derive the NATS store configuration for this runtime config.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::MissingField`] if `org_name` is empty.
    pub fn nats_store_config(&self) -> Result<NatsStoreConfig, ConfigError> {
        Ok(
            NatsStoreConfig::for_org(&self.org_name, self.nats_url.clone())?
                .with_credentials_path(self.nats_creds.clone()),
        )
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

    #[test]
    fn runtime_config_defaults_to_pgno_and_anonymous_nats() {
        let cfg = RuntimeConfig::new("org", false, 8, PathBuf::from("s")).unwrap();

        assert_eq!(cfg.pardosa_backend, PardosaBackend::Pgno);
        assert_eq!(cfg.nats_url, DEFAULT_NATS_URL);
        assert!(cfg.nats_creds.is_none());
    }

    #[test]
    fn nats_store_config_uses_injective_hex_org_token() {
        let my_org = NatsStoreConfig::for_org("my org", DEFAULT_NATS_URL).unwrap();
        let my_dash_org = NatsStoreConfig::for_org("my-org", DEFAULT_NATS_URL).unwrap();
        let dotted = NatsStoreConfig::for_org("a.b", DEFAULT_NATS_URL).unwrap();
        let dashed = NatsStoreConfig::for_org("a-b", DEFAULT_NATS_URL).unwrap();

        assert_eq!(my_org.stream_name, "gh-report-org_6d79206f7267");
        assert_eq!(my_org.subject, "gh-report.org_6d79206f7267.events");
        assert_eq!(my_org.durable_consumer, "gh-report-org_6d79206f7267");
        assert_ne!(my_org.stream_name, my_dash_org.stream_name);
        assert_ne!(my_org.subject, my_dash_org.subject);
        assert_ne!(dotted.stream_name, dashed.stream_name);
        assert_ne!(dotted.subject, dashed.subject);
        assert!(!my_org.subject.contains(' '));
        assert!(!my_org.subject.contains('*'));
        assert!(!my_org.subject.contains('>'));
    }

    #[test]
    fn nats_store_config_derives_distinct_org_stream() {
        let repo = NatsStoreConfig::for_org("my org", DEFAULT_NATS_URL).unwrap();
        let org = repo.org_events();

        assert_eq!(org.stream_name, "gh-report-org_6d79206f7267-org");
        assert_eq!(org.subject, "gh-report.org_6d79206f7267.org.events");
        assert_eq!(org.durable_consumer, "gh-report-org_6d79206f7267-org");
        assert_ne!(repo.stream_name, org.stream_name);
        assert_ne!(repo.subject, org.subject);
    }

    #[test]
    fn nats_store_config_threads_credentials_path() {
        let path = PathBuf::from("/var/secrets/nats.creds");
        let cfg = NatsStoreConfig::for_org("my org", DEFAULT_NATS_URL)
            .unwrap()
            .with_credentials_path(Some(path.clone()));

        assert_eq!(cfg.credentials_path, Some(path));
    }

    #[test]
    fn nats_store_config_org_events_carries_credentials_path() {
        let path = PathBuf::from("/var/secrets/nats.creds");
        let repo = NatsStoreConfig::for_org("my org", DEFAULT_NATS_URL)
            .unwrap()
            .with_credentials_path(Some(path.clone()));
        let org = repo.org_events();

        assert_eq!(org.credentials_path, Some(path));
    }

    #[test]
    fn nats_store_config_rejects_empty_org() {
        let result = NatsStoreConfig::for_org("", DEFAULT_NATS_URL);

        assert!(matches!(result, Err(ConfigError::MissingField { .. })));
    }

    #[test]
    fn runtime_config_derives_default_nats_store_config() {
        let cfg = RuntimeConfig::new("org", false, 8, PathBuf::from("store")).unwrap();

        assert_eq!(cfg.nats_url, DEFAULT_NATS_URL);
        assert_eq!(
            cfg.nats_store_config().unwrap().stream_name,
            "gh-report-org_6f7267"
        );
        assert!(cfg.nats_store_config().unwrap().credentials_path.is_none());
    }

    #[test]
    fn runtime_config_carries_nats_creds_to_store_config() {
        let path = PathBuf::from("/var/secrets/nats.creds");
        let mut cfg = RuntimeConfig::new("org", false, 8, PathBuf::from("store")).unwrap();
        cfg.pardosa_backend = PardosaBackend::Nats;
        cfg.nats_creds = Some(path.clone());

        assert_eq!(
            cfg.nats_store_config().unwrap().credentials_path,
            Some(path)
        );
    }
}
