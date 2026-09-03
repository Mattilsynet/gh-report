//! Structured emission of the connected NATS broker's identity.
//!
//! `pardosa-nats` observes the broker but cannot log (substrate ring
//! purity, `AGENTS.md`); this module is the adapter-ring sink it hands
//! observations to. Fields are emitted individually so each is queryable
//! as `jsonPayload.<field>` in Cloud Logging, and are drawn from an
//! explicit allowlist rather than by serialising `ServerInfo` wholesale
//! (SEC-0007:R1/R2/R4, COM-0019:R5).
//!
//! Each record also carries `nats_connect_generation`, the broker
//! connection counter the identity was read from. `async-nats` announces
//! connection events over a bounded channel that drops on overflow, so a
//! burst of reconnects can be coalesced into fewer records than
//! connections. The generation sequence makes that visible as a gap
//! rather than letting a flap look like a quiet period.

use std::sync::{Arc, Mutex, PoisonError};

/// Level-and-payload decision for one observation, kept separate from the
/// `tracing` emission so the decision is unit-testable without a subscriber.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum VersionTransition {
    /// First broker version seen by this process.
    FirstSeen,
    /// Reconnected to a broker reporting the same version as before.
    Unchanged,
    /// The broker version changed underneath a live process — the event
    /// that would invalidate PGN-0016:22 / PGN-0024:41.
    Changed { previous: String },
}

/// Tracks the last broker version seen and emits one structured record per
/// established connection.
#[derive(Debug, Default)]
pub struct NatsServerInfoLogger {
    last_version: Mutex<Option<String>>,
}

impl NatsServerInfoLogger {
    /// Build a logger with no broker version observed yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adapt this logger into the observer the substrate invokes on every
    /// established connection, including `async-nats`-internal reconnects.
    #[must_use]
    pub fn into_observer(self: Arc<Self>) -> pardosa_nats::ServerInfoObserver {
        pardosa_nats::ServerInfoObserver::new(Arc::new(move |info, connect_generation| {
            self.observe(info, connect_generation);
        }))
    }

    fn classify(&self, version: &str) -> VersionTransition {
        let mut guard = self
            .last_version
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        match guard.replace(version.to_owned()) {
            None => VersionTransition::FirstSeen,
            Some(previous) if previous == version => VersionTransition::Unchanged,
            Some(previous) => VersionTransition::Changed { previous },
        }
    }

    fn observe(&self, info: &pardosa_nats::ServerInfo, connect_generation: u64) {
        match self.classify(&info.version) {
            VersionTransition::Changed { previous } => tracing::warn!(
                previous_server_version = %previous,
                nats_connect_generation = connect_generation,
                server_version = %info.version,
                server_go = %info.go,
                server_host = %info.host,
                server_port = info.port,
                server_jetstream = info.jetstream,
                server_max_payload = info.max_payload,
                server_proto = info.proto,
                server_id = %info.server_id,
                server_name = %info.server_name,
                server_cluster = ?info.cluster,
                server_headers = info.headers,
                server_auth_required = info.auth_required,
                server_tls_required = info.tls_required,
                "nats broker version changed"
            ),
            VersionTransition::FirstSeen | VersionTransition::Unchanged => tracing::info!(
                nats_connect_generation = connect_generation,
                server_version = %info.version,
                server_go = %info.go,
                server_host = %info.host,
                server_port = info.port,
                server_jetstream = info.jetstream,
                server_max_payload = info.max_payload,
                server_proto = info.proto,
                server_id = %info.server_id,
                server_name = %info.server_name,
                server_cluster = ?info.cluster,
                server_headers = info.headers,
                server_auth_required = info.auth_required,
                server_tls_required = info.tls_required,
                "nats broker connected"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Arc, Mutex, NatsServerInfoLogger, PoisonError, VersionTransition};
    use tracing::Level;
    use tracing_subscriber::layer::{Context, SubscriberExt};

    const FORBIDDEN_SUBSTRINGS: [&str; 5] = [
        "10.9.9.9",
        "supersecretnonce",
        "internal-a",
        "client_ip",
        "nonce",
    ];

    #[derive(Clone, Debug)]
    struct Captured {
        level: Level,
        message: String,
        fields: Vec<(String, String)>,
    }

    impl Captured {
        fn field(&self, name: &str) -> Option<&str> {
            self.fields
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.as_str())
        }

        fn rendered(&self) -> String {
            format!("{self:?}")
        }
    }

    #[derive(Default)]
    struct Visitor {
        message: String,
        fields: Vec<(String, String)>,
    }

    impl tracing::field::Visit for Visitor {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            let rendered = format!("{value:?}");
            if field.name() == "message" {
                self.message = rendered;
            } else {
                self.fields.push((field.name().to_owned(), rendered));
            }
        }
    }

    struct CaptureLayer {
        events: Arc<Mutex<Vec<Captured>>>,
    }

    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for CaptureLayer {
        fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
            let mut visitor = Visitor::default();
            event.record(&mut visitor);
            self.events
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(Captured {
                    level: *event.metadata().level(),
                    message: visitor.message,
                    fields: visitor.fields,
                });
        }
    }

    fn capture(f: impl FnOnce()) -> Vec<Captured> {
        let events = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::Registry::default().with(CaptureLayer {
            events: Arc::clone(&events),
        });
        tracing::subscriber::with_default(subscriber, f);
        let captured = events.lock().unwrap_or_else(PoisonError::into_inner);
        captured.clone()
    }

    fn server_info(version: &str) -> pardosa_nats::ServerInfo {
        pardosa_nats::ServerInfo {
            version: version.to_owned(),
            go: "go1.25".to_owned(),
            host: "10.0.0.1".to_owned(),
            port: 4222,
            jetstream: true,
            max_payload: 1_048_576,
            proto: 1,
            server_id: "NDXPROD".to_owned(),
            server_name: "prod-nats-0".to_owned(),
            cluster: Some("prod".to_owned()),
            headers: true,
            auth_required: true,
            tls_required: true,
            client_ip: "10.9.9.9".to_owned(),
            nonce: "supersecretnonce".to_owned(),
            connect_urls: vec!["nats://internal-a:4222".to_owned()],
            ..pardosa_nats::ServerInfo::default()
        }
    }

    #[test]
    fn first_connect_is_classified_first_seen() {
        let logger = NatsServerInfoLogger::new();

        assert_eq!(logger.classify("2.14.5"), VersionTransition::FirstSeen);
    }

    #[test]
    fn reconnect_with_same_version_is_classified_unchanged() {
        let logger = NatsServerInfoLogger::new();
        let _ = logger.classify("2.14.5");

        assert_eq!(logger.classify("2.14.5"), VersionTransition::Unchanged);
    }

    #[test]
    fn version_change_across_reconnect_carries_old_and_new_then_rebaselines() {
        let logger = NatsServerInfoLogger::new();
        let _ = logger.classify("2.14.5");

        assert_eq!(
            logger.classify("2.14.6"),
            VersionTransition::Changed {
                previous: "2.14.5".to_owned()
            },
            "a platform-side upgrade under a live process must stay distinguishable"
        );
        assert_eq!(
            logger.classify("2.14.6"),
            VersionTransition::Unchanged,
            "the new version becomes the baseline for subsequent reconnects"
        );
    }

    #[test]
    fn first_connect_emits_every_allowlisted_field_at_info() {
        let events = capture(|| {
            NatsServerInfoLogger::new().observe(&server_info("2.14.5"), 1);
        });

        assert_eq!(events.len(), 1);
        let record = &events[0];
        assert_eq!(record.level, Level::INFO);
        assert_eq!(record.message, "nats broker connected");
        assert_eq!(record.field("server_version"), Some("2.14.5"));
        assert_eq!(record.field("server_go"), Some("go1.25"));
        assert_eq!(record.field("server_host"), Some("10.0.0.1"));
        assert_eq!(record.field("server_port"), Some("4222"));
        assert_eq!(record.field("server_jetstream"), Some("true"));
        assert_eq!(record.field("server_max_payload"), Some("1048576"));
        assert_eq!(record.field("server_proto"), Some("1"));
        assert_eq!(record.field("server_id"), Some("NDXPROD"));
        assert_eq!(record.field("server_name"), Some("prod-nats-0"));
        assert_eq!(record.field("server_cluster"), Some("Some(\"prod\")"));
        assert_eq!(record.field("server_headers"), Some("true"));
        assert_eq!(record.field("server_auth_required"), Some("true"));
        assert_eq!(record.field("server_tls_required"), Some("true"));
        assert_eq!(record.field("nats_connect_generation"), Some("1"));
        assert_eq!(
            record.fields.len(),
            14,
            "the allowlist is exactly 13 broker fields plus the connect generation; \
             a 15th means something leaked in"
        );
    }

    #[test]
    fn a_coalesced_flap_is_visible_as_a_gap_in_the_generation_sequence() {
        let logger = NatsServerInfoLogger::new();
        let events = capture(|| {
            logger.observe(&server_info("2.14.5"), 1);
            logger.observe(&server_info("2.14.5"), 4);
        });

        assert_eq!(
            events
                .iter()
                .map(|e| e.field("nats_connect_generation").unwrap_or_default())
                .collect::<Vec<_>>(),
            vec!["1", "4"],
            "generations 2 and 3 were coalesced away by the event channel; the gap is \
             what makes that observable instead of silently smoothed"
        );
    }

    #[test]
    fn reconnect_with_same_version_stays_at_info() {
        let logger = NatsServerInfoLogger::new();
        let events = capture(|| {
            logger.observe(&server_info("2.14.5"), 1);
            logger.observe(&server_info("2.14.5"), 2);
        });

        assert_eq!(events.len(), 2);
        assert!(
            events.iter().all(|e| e.level == Level::INFO),
            "steady-state reconnects must not inflate WARN volume (COM-0031:R1/R4)"
        );
        assert!(
            events
                .iter()
                .all(|e| e.field("previous_server_version").is_none())
        );
    }

    #[test]
    fn reconnect_with_changed_version_warns_with_old_and_new() {
        let logger = NatsServerInfoLogger::new();
        let events = capture(|| {
            logger.observe(&server_info("2.14.5"), 1);
            logger.observe(&server_info("2.14.6"), 2);
        });

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].level, Level::INFO);

        let changed = &events[1];
        assert_eq!(changed.level, Level::WARN);
        assert_eq!(changed.message, "nats broker version changed");
        assert_eq!(changed.field("previous_server_version"), Some("2.14.5"));
        assert_eq!(changed.field("server_version"), Some("2.14.6"));
    }

    #[test]
    fn no_forbidden_field_appears_in_any_emitted_record() {
        let logger = NatsServerInfoLogger::new();
        let events = capture(|| {
            logger.observe(&server_info("2.14.5"), 1);
            logger.observe(&server_info("2.14.6"), 2);
        });

        assert_eq!(events.len(), 2);
        for record in &events {
            let rendered = record.rendered();
            for forbidden in FORBIDDEN_SUBSTRINGS {
                assert!(
                    !rendered.contains(forbidden),
                    "forbidden value {forbidden} leaked into an emitted record: {rendered}"
                );
            }
            assert!(record.field("connect_urls").is_none());
        }
    }

    #[test]
    fn absent_server_info_emits_no_record_at_all() {
        let logger = Arc::new(NatsServerInfoLogger::new());
        let observer = Arc::clone(&logger).into_observer();
        let absent: Option<pardosa_nats::ServerInfo> = None;

        let events = capture(|| {
            if let Some(info) = absent.as_ref() {
                observer.observe(info, 1);
            }
        });

        assert!(
            events.is_empty(),
            "try_server_info() == None must never be defaulted into a version:\"\" record"
        );
        assert_eq!(
            *logger
                .last_version
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
            None,
            "a skipped observation must not poison the version baseline"
        );
    }

    #[test]
    fn observer_adapter_forwards_to_the_logger() {
        let logger = Arc::new(NatsServerInfoLogger::new());
        let observer = Arc::clone(&logger).into_observer();

        let events = capture(|| {
            observer.observe(&server_info("2.14.5"), 1);
            observer.observe(&server_info("2.14.6"), 2);
        });

        assert_eq!(events.len(), 2);
        assert_eq!(events[1].level, Level::WARN);
    }
}
