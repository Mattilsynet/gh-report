use crate::error::{BackendError, PardosaError};
use pardosa_nats::JetStreamRuntimeError;
use sha2::{Digest, Sha256};
use std::error::Error;
use std::path::Path;

/// Operator-facing `NATS` startup and `JetStream` failure categories.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NatsFailureClass {
    /// `NATS` account, subject, or stream operation was rejected by authorization.
    AuthzViolation,
    /// `JetStream` stream provisioning was denied or unavailable.
    JetStreamProvisioningDenied,
    /// Credential material appears expired, stale, or invalid.
    CredsStaleInvalid,
    /// TLS connection setup failed.
    TlsConnection,
    /// Endpoint reachability or connection setup failed.
    ConnectionRefused,
    /// Configured credentials path is missing or unreadable.
    SecretPath,
    /// No known NATS failure category matched.
    Unknown,
}

impl NatsFailureClass {
    /// Stable snake-case category label for logs and operator output.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AuthzViolation => "authz_violation",
            Self::JetStreamProvisioningDenied => "jetstream_provisioning_denied",
            Self::CredsStaleInvalid => "creds_stale_invalid",
            Self::TlsConnection => "tls_connection",
            Self::ConnectionRefused => "connection_refused",
            Self::SecretPath => "secret_path",
            Self::Unknown => "unknown",
        }
    }
}

/// Remediation hint paired with a [`NatsFailureClass`].
#[must_use]
pub const fn nats_failure_remediation(class: NatsFailureClass) -> &'static str {
    match class {
        NatsFailureClass::AuthzViolation => {
            "check the NATS account permissions for the configured subject; if credentials are byte-valid, suspect a connection-origin or network-identity mismatch at the connection boundary"
        }
        NatsFailureClass::JetStreamProvisioningDenied => {
            "enable JetStream for the NATS account, grant stream and subject create permissions, and verify stream configuration"
        }
        NatsFailureClass::CredsStaleInvalid => {
            "rotate the NATS credentials secret and restart the service"
        }
        NatsFailureClass::TlsConnection => "check NATS TLS endpoint and trust configuration",
        NatsFailureClass::ConnectionRefused => {
            "check NATS endpoint reachability and service status"
        }
        NatsFailureClass::SecretPath => "check the configured NATS credentials secret mount path",
        NatsFailureClass::Unknown => "inspect the preserved error_chain and NATS service logs",
    }
}

/// Emit startup diagnostics for a `NATS` endpoint and optional credentials file.
pub fn emit_nats_connect_diagnostics(nats_url: &str, credentials_path: Option<&Path>) {
    let creds_path = credentials_path
        .map(Path::display)
        .map(|path| path.to_string());
    let creds_path_display = creds_path.as_deref().unwrap_or("");
    let creds = credentials_path.map_or(CredsDiagnostic::missing_path(), creds_diagnostic);
    tracing::info!(
        nats_url = nats_url,
        creds_path = creds_path_display,
        creds_exists = creds.exists,
        creds_len = creds.len,
        creds_sha256_prefix = creds.sha256_prefix.as_deref().unwrap_or(""),
        "nats connect diagnostics"
    );
}

struct CredsDiagnostic {
    exists: bool,
    len: u64,
    sha256_prefix: Option<String>,
}

impl CredsDiagnostic {
    const fn missing_path() -> Self {
        Self {
            exists: false,
            len: 0,
            sha256_prefix: None,
        }
    }
}

fn creds_diagnostic(path: &Path) -> CredsDiagnostic {
    let metadata = path.metadata();
    let exists = metadata.is_ok();
    let len = metadata.as_ref().map_or(0, std::fs::Metadata::len);
    match std::fs::read(path) {
        Ok(bytes) => {
            let digest = Sha256::digest(&bytes);
            CredsDiagnostic {
                exists: true,
                len: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
                sha256_prefix: Some(hex_prefix_8(&digest)),
            }
        }
        Err(_) => CredsDiagnostic {
            exists,
            len,
            sha256_prefix: None,
        },
    }
}

fn hex_prefix_8(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(8);
    for byte in bytes.iter().take(4) {
        out.push(char::from(HEX[usize::from(byte >> 4)]));
        out.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    out
}

/// Classify a public `pardosa` or `JetStream` error chain into a `NATS` category.
#[must_use]
pub fn classify_nats_failure(error: &(dyn Error + 'static)) -> NatsFailureClass {
    let mut current = Some(error);
    while let Some(error) = current {
        let class = classify_nats_failure_typed(error);
        if class != NatsFailureClass::Unknown {
            return class;
        }
        let class = classify_nats_failure_display(&error.to_string());
        if class != NatsFailureClass::Unknown {
            return class;
        }
        let class = classify_nats_failure_display(&format!("{error:?}"));
        if class != NatsFailureClass::Unknown {
            return class;
        }
        if let Some(io) = error.downcast_ref::<std::io::Error>()
            && let Some(inner) = io.get_ref()
        {
            let class = classify_nats_failure(inner);
            if class != NatsFailureClass::Unknown {
                return class;
            }
        }
        current = error.source();
    }
    NatsFailureClass::Unknown
}

fn classify_nats_failure_typed(error: &(dyn Error + 'static)) -> NatsFailureClass {
    if let Some(backend) = error.downcast_ref::<BackendError>() {
        return classify_backend_error(backend);
    }
    if let Some(pardosa) = error.downcast_ref::<PardosaError>() {
        return classify_pardosa_error(pardosa);
    }
    if let Some(jetstream) = error.downcast_ref::<JetStreamRuntimeError>() {
        return classify_jetstream_runtime_error(jetstream);
    }
    NatsFailureClass::Unknown
}

fn classify_backend_error(error: &BackendError) -> NatsFailureClass {
    match error {
        BackendError::Timeout { .. } => NatsFailureClass::ConnectionRefused,
        BackendError::Connect { source, .. }
        | BackendError::Publish { source, .. }
        | BackendError::ConcurrencyConflict { source, .. }
        | BackendError::Replay { source, .. } => classify_nats_failure(source.as_ref()),
        BackendError::RuntimeFailure { .. } | BackendError::PublisherBacklog { .. } => {
            NatsFailureClass::Unknown
        }
    }
}

fn classify_pardosa_error(error: &PardosaError) -> NatsFailureClass {
    match error {
        PardosaError::ConcurrencyConflict { source, .. } => classify_nats_failure(source.as_ref()),
        _ => NatsFailureClass::Unknown,
    }
}

fn classify_jetstream_runtime_error(error: &JetStreamRuntimeError) -> NatsFailureClass {
    match error {
        JetStreamRuntimeError::Timeout { .. } => NatsFailureClass::ConnectionRefused,
        JetStreamRuntimeError::Connect { source }
        | JetStreamRuntimeError::Publish { source }
        | JetStreamRuntimeError::Replay { source } => classify_nats_failure(source.as_ref()),
        JetStreamRuntimeError::WrongLastSequence { source, .. } => {
            classify_nats_failure(source.as_ref())
        }
        JetStreamRuntimeError::Detached | _ => NatsFailureClass::Unknown,
    }
}

const fn classify_jetstream_error_code(error_code: u64) -> NatsFailureClass {
    match error_code {
        10023 | 10035 | 10039 => NatsFailureClass::JetStreamProvisioningDenied,
        _ => NatsFailureClass::Unknown,
    }
}

fn classify_nats_failure_display(display: &str) -> NatsFailureClass {
    let lower = display.to_ascii_lowercase();
    if lower.contains("authorization violation")
        || lower.contains("permissions violation")
        || lower.contains("permission violation")
        || lower.contains("not authorized")
    {
        return NatsFailureClass::AuthzViolation;
    }
    for error_code in [10023, 10035, 10039] {
        if contains_error_code(&lower, error_code) {
            return classify_jetstream_error_code(error_code);
        }
    }
    if lower.contains("invalid credentials")
        || lower.contains("stale credentials")
        || lower.contains("expired") && lower.contains("credential")
        || lower.contains("jwt") && (lower.contains("expired") || lower.contains("invalid"))
    {
        return NatsFailureClass::CredsStaleInvalid;
    }
    if lower.contains("no such file")
        || lower.contains("not found") && lower.contains("credential")
        || lower.contains("credentials file")
        || lower.contains("secret") && lower.contains("path")
    {
        return NatsFailureClass::SecretPath;
    }
    if lower.contains("tls")
        || lower.contains("certificate")
        || lower.contains("cert") && lower.contains("invalid")
        || lower.contains("handshake")
    {
        return NatsFailureClass::TlsConnection;
    }
    if lower.contains("connection refused")
        || lower.contains("connection reset")
        || lower.contains("connection aborted")
        || lower.contains("operation timed out")
        || lower.contains("timed out") && lower.contains("connect")
    {
        return NatsFailureClass::ConnectionRefused;
    }
    NatsFailureClass::Unknown
}

fn contains_error_code(value: &str, error_code: u64) -> bool {
    let needle = error_code.to_string();
    value.match_indices(&needle).any(|(idx, _)| {
        let before = value[..idx].chars().next_back();
        let after = value[idx + needle.len()..].chars().next();
        before.is_none_or(|c| !c.is_ascii_digit()) && after.is_none_or(|c| !c.is_ascii_digit())
    })
}

/// Render an error chain as redacted JSON objects.
#[must_use]
pub fn error_chain_json(error: &(dyn Error + 'static)) -> String {
    let mut chain = String::from("[");
    let mut current = Some(error);
    let mut level = 0_u64;
    while let Some(error) = current {
        if level > 0 {
            chain.push(',');
        }
        let display = json_escape(&redact_nats_credentials(&error.to_string()));
        let debug = json_escape(&redact_nats_credentials(&format!("{error:?}")));
        chain.push_str("{\"level\":");
        chain.push_str(&level.to_string());
        chain.push_str(",\"display\":\"");
        chain.push_str(&display);
        chain.push_str("\",\"debug\":\"");
        chain.push_str(&debug);
        chain.push_str("\"}");
        current = error.source();
        level += 1;
    }
    chain.push(']');
    chain
}

/// Replace PEM-style NATS credential blocks with a fixed redaction marker.
#[must_use]
pub fn redact_nats_credentials(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut rest = value;
    while let Some((begin, begin_fence)) = find_pem_begin(rest) {
        output.push_str(&rest[..begin]);
        output.push_str("[redacted nats credential block]");
        let after_begin = &rest[begin + begin_fence.fence_len..];
        if let Some(end) = find_pem_end(after_begin, begin_fence.label) {
            rest = &after_begin[end..];
        } else {
            rest = "";
        }
    }
    output.push_str(rest);
    output
}

#[derive(Clone, Copy)]
struct PemBeginFence<'a> {
    label: &'a str,
    fence_len: usize,
}

fn find_pem_begin(value: &str) -> Option<(usize, PemBeginFence<'_>)> {
    ["-----BEGIN ", "------BEGIN "]
        .into_iter()
        .filter_map(|prefix| {
            let begin = value.find(prefix)?;
            let after_prefix = &value[begin + prefix.len()..];
            let label_end = after_prefix.find('-')?;
            let label = &after_prefix[..label_end];
            let trailing = &after_prefix[label_end..];
            let trailing_len = pem_fence_dash_len(trailing)?;
            let fence_len = prefix.len() + label.len() + trailing_len;
            Some((begin, PemBeginFence { label, fence_len }))
        })
        .min_by_key(|(begin, _)| *begin)
}

fn find_pem_end(value: &str, label: &str) -> Option<usize> {
    let five_dash = format!("-----END {label}-----");
    let six_dash = format!("------END {label}------");
    [five_dash.as_str(), six_dash.as_str()]
        .into_iter()
        .filter_map(|fence| value.find(fence).map(|start| start + fence.len()))
        .min()
}

fn pem_fence_dash_len(value: &str) -> Option<usize> {
    if value.starts_with("------") {
        Some(6)
    } else if value.starts_with("-----") {
        Some(5)
    } else {
        None
    }
}

fn json_escape(value: &str) -> String {
    value
        .chars()
        .flat_map(char::escape_default)
        .collect::<String>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tracing::field::{Field, Visit};
    use tracing_subscriber::layer::{Context, SubscriberExt};

    struct CapturedEvents {
        lines: Arc<Mutex<Vec<String>>>,
    }

    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for CapturedEvents {
        fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
            let mut visitor = CapturedFields::default();
            event.record(&mut visitor);
            self.lines
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(visitor.line);
        }
    }

    #[derive(Default)]
    struct CapturedFields {
        line: String,
    }

    impl CapturedFields {
        fn push(&mut self, field: &Field, value: impl std::fmt::Display) {
            self.line.push_str(field.name());
            self.line.push('=');
            self.line.push_str(&value.to_string());
            self.line.push(';');
        }
    }

    impl Visit for CapturedFields {
        fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
            self.push(field, format_args!("{value:?}"));
        }

        fn record_str(&mut self, field: &Field, value: &str) {
            self.push(field, value);
        }

        fn record_bool(&mut self, field: &Field, value: bool) {
            self.push(field, value);
        }

        fn record_u64(&mut self, field: &Field, value: u64) {
            self.push(field, value);
        }
    }

    fn capture_events(f: impl FnOnce()) -> String {
        let lines = Arc::new(Mutex::new(Vec::new()));
        let layer = CapturedEvents {
            lines: Arc::clone(&lines),
        };
        let subscriber = tracing_subscriber::Registry::default().with(layer);
        tracing::subscriber::with_default(subscriber, || {
            tracing::callsite::rebuild_interest_cache();
            f();
            tracing::callsite::rebuild_interest_cache();
        });
        lines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .join("\n")
    }

    fn assert_nats_failure_class(message: &'static str, expected: &'static str) {
        let err = std::io::Error::other(message);

        assert_eq!(classify_nats_failure(&err).as_str(), expected);
    }

    fn boxed_source(msg: &str) -> Box<dyn Error + Send + Sync + 'static> {
        Box::new(std::io::Error::other(msg))
    }

    fn jetstream_create_stream_error(
        error_code: u64,
    ) -> async_nats::jetstream::context::CreateStreamError {
        let error = serde_json::from_value::<async_nats::jetstream::Error>(serde_json::json!({
            "code": 503,
            "err_code": error_code,
            "description": "unstable server description"
        }))
        .expect("synthetic JetStream error should deserialize");

        error.into()
    }

    #[test]
    fn nats_failure_classifies_jetstream_provisioning_denied_by_error_code() {
        let err = BackendError::Connect {
            op: crate::error::BackendOp::Sync,
            source: Box::new(jetstream_create_stream_error(10039)),
        };
        let class = classify_nats_failure(&err);

        assert_eq!(class.as_str(), "jetstream_provisioning_denied");
        assert_eq!(
            nats_failure_remediation(class),
            "enable JetStream for the NATS account, grant stream and subject create permissions, and verify stream configuration"
        );

        let output = capture_events(|| {
            let error_chain = error_chain_json(&err);
            let error_display = redact_nats_credentials(&err.to_string());
            let nats_failure_class = class.as_str();
            let nats_failure_remediation = nats_failure_remediation(class);
            tracing::error!(
                nats_failure_class = nats_failure_class,
                nats_failure_remediation = nats_failure_remediation,
                error_chain = error_chain.as_str(),
                error = error_display.as_str(),
                "persistence error chain captured before flattening"
            );
        });
        assert!(output.contains("nats_failure_class=jetstream_provisioning_denied"));
        assert!(output.contains(
            "nats_failure_remediation=enable JetStream for the NATS account, grant stream and subject create permissions, and verify stream configuration"
        ));
    }

    #[test]
    fn nats_failure_classifies_public_jetstream_runtime_error_sources() {
        let err = JetStreamRuntimeError::Connect {
            source: boxed_source("nats: authorization violation"),
        };

        assert_eq!(classify_nats_failure(&err).as_str(), "authz_violation");
    }

    #[test]
    fn nats_failure_classifies_authz_violation() {
        assert_nats_failure_class("nats: authorization violation", "authz_violation");
    }

    #[test]
    fn nats_failure_remediation_names_origin_class_for_authz_violation() {
        assert_eq!(
            nats_failure_remediation(NatsFailureClass::AuthzViolation),
            "check the NATS account permissions for the configured subject; if credentials are byte-valid, suspect a connection-origin or network-identity mismatch at the connection boundary"
        );
    }

    #[test]
    fn nats_failure_classifies_creds_stale_invalid() {
        assert_nats_failure_class(
            "nats: invalid credentials jwt expired",
            "creds_stale_invalid",
        );
    }

    #[test]
    fn nats_failure_classifies_tls_connection() {
        assert_nats_failure_class(
            "tls handshake failed: invalid certificate",
            "tls_connection",
        );
    }

    #[test]
    fn nats_failure_classifies_connection_refused() {
        assert_nats_failure_class(
            "connect error: Connection refused (os error 61)",
            "connection_refused",
        );
    }

    #[test]
    fn nats_failure_classifies_secret_path() {
        assert_nats_failure_class(
            "credentials file /var/secrets/nats.creds: No such file or directory",
            "secret_path",
        );
    }

    #[test]
    fn nats_failure_classifies_unknown() {
        assert_nats_failure_class("backend reported an unmapped startup error", "unknown");
    }

    #[test]
    fn redact_nats_credentials_truncates_unclosed_pem_block() {
        let output = redact_nats_credentials(
            "prefix\n-----BEGIN USER NKEY SEED-----\nsecret\nvisible text after missing fence",
        );

        assert_eq!(output, "prefix\n[redacted nats credential block]");
    }

    #[test]
    fn error_chain_json_redacts_secret_blocks() {
        let secret = "super-secret-material-for-test";
        let message = format!(
            "nats: invalid credentials\n-----BEGIN USER NKEY SEED-----\n{secret}\n-----END USER NKEY SEED-----"
        );
        let err = std::io::Error::other(message);

        let output = error_chain_json(&err);

        assert!(output.contains("[redacted nats credential block]"));
        assert!(!output.contains(secret));
    }

    #[test]
    fn nats_connect_diagnostics_log_creds_fingerprint_without_secret_bytes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("user.creds");
        let secret = "super-secret-material-for-test";
        std::fs::write(&path, secret).expect("write creds");

        let output = capture_events(|| {
            emit_nats_connect_diagnostics("tls://connect.nats.mattilsynet.io:4222", Some(&path));
        });

        assert!(output.contains("nats_url=tls://connect.nats.mattilsynet.io:4222"));
        assert!(output.contains("creds_exists=true"));
        assert!(output.contains(&format!("creds_len={};", secret.len())));
        assert!(output.contains("creds_sha256_prefix="));
        assert!(!output.contains(secret));
    }
}
