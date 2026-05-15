//! Server configuration with typestate builder pattern.
//!
//! Absorbed under mission `absorb-server-1778695800` (P1-A.5.2).
//! Byte-for-byte port; only doc-example crate paths and the
//! `crate::server::build_router` reference were rewritten.
//!
//! Configuration flows through two types:
//!
//! - [`ServerConfigBuilder`] — mutable builder with setter methods.
//!   Created via [`ServerConfig::builder()`].
//! - [`ValidatedConfig`] — immutable, validated configuration.
//!   Created only via [`ServerConfigBuilder::build()`].
//!
//! This pattern makes invalid configuration unrepresentable:
//! [`super::server::build_router`] accepts `&ValidatedConfig`, so
//! callers cannot pass unchecked values.

use thiserror::Error;

/// Typed errors for configuration validation.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum ConfigError {
    /// `concurrency_limit` was set to 0.
    #[error("concurrency_limit must be >= 1")]
    ConcurrencyLimitZero,

    /// `concurrency_limit` exceeds Tokio's semaphore capacity.
    #[error("concurrency_limit exceeds Semaphore::MAX_PERMITS")]
    ConcurrencyLimitOverflow,

    /// `ws_max_connections` was set to 0.
    #[error("ws_max_connections must be >= 1")]
    WsMaxConnectionsZero,

    /// `ws_max_connections` exceeds Tokio's semaphore capacity.
    #[error("ws_max_connections exceeds Semaphore::MAX_PERMITS")]
    WsMaxConnectionsOverflow,

    /// `max_request_body_bytes` was set to 0.
    #[error("max_request_body_bytes must be >= 1")]
    MaxRequestBodyBytesZero,

    /// `error_page_key` was empty.
    #[error("error_page_key must not be empty")]
    ErrorPageKeyEmpty,

    /// `error_page_key` started with `/`.
    #[error("error_page_key must not start with '/'")]
    ErrorPageKeyLeadingSlash,

    /// `error_page_key` contained `..`.
    #[error("error_page_key must not contain '..'")]
    ErrorPageKeyTraversal,

    /// `error_page_key` contained a null byte.
    #[error("error_page_key must not contain null bytes")]
    ErrorPageKeyNullByte,

    /// `error_page_key` contained a backslash.
    #[error("error_page_key must not contain backslashes")]
    ErrorPageKeyBackslash,

    /// `csp_override` contained non-ASCII characters.
    #[error("csp_override must be valid ASCII")]
    CspNotAscii,

    /// `csp_override` contained CR or LF characters.
    ///
    /// HTTP header values must not contain `\r` or `\n`. Allowing these
    /// would cause `HeaderValue::from_str()` to panic at router
    /// construction time.
    #[error("csp_override must not contain CR or LF characters")]
    CspContainsCrlf,
}

/// Entry point for creating server configuration.
#[derive(Debug)]
#[non_exhaustive]
pub struct ServerConfig;

impl ServerConfig {
    /// Create a new [`ServerConfigBuilder`] with default values.
    ///
    /// Defaults:
    /// - `concurrency_limit`: 1024
    /// - `max_request_body_bytes`: 1024
    /// - `ws_max_connections`: 200
    /// - `csp_override`: `None`
    /// - `error_page_key`: `None`
    #[must_use]
    pub fn builder() -> ServerConfigBuilder {
        ServerConfigBuilder::default()
    }
}

/// Builder for server configuration.
#[derive(Debug, Clone)]
pub struct ServerConfigBuilder {
    concurrency_limit: usize,
    max_request_body_bytes: usize,
    ws_max_connections: usize,
    csp_override: Option<String>,
    error_page_key: Option<String>,
}

impl Default for ServerConfigBuilder {
    fn default() -> Self {
        Self {
            concurrency_limit: 1024,
            max_request_body_bytes: 1024,
            ws_max_connections: 200,
            csp_override: None,
            error_page_key: None,
        }
    }
}

impl ServerConfigBuilder {
    /// Maximum concurrent in-flight HTTP requests (excluding WebSocket).
    #[must_use]
    pub fn concurrency_limit(mut self, limit: usize) -> Self {
        self.concurrency_limit = limit;
        self
    }

    /// Maximum request body size in bytes.
    #[must_use]
    pub fn max_request_body_bytes(mut self, limit: usize) -> Self {
        self.max_request_body_bytes = limit;
        self
    }

    /// Maximum concurrent WebSocket connections.
    #[must_use]
    pub fn ws_max_connections(mut self, limit: usize) -> Self {
        self.ws_max_connections = limit;
        self
    }

    /// Override the default Content-Security-Policy header.
    #[must_use]
    pub fn csp_override(mut self, csp: impl Into<String>) -> Self {
        self.csp_override = Some(csp.into());
        self
    }

    /// Set the cache key for a custom error page (e.g., `"404.html"`).
    #[must_use]
    pub fn error_page_key(mut self, key: impl Into<String>) -> Self {
        self.error_page_key = Some(key.into());
        self
    }

    /// Validate and build the configuration.
    ///
    /// # Errors
    ///
    /// Returns `Err(ConfigError)` if any field has an invalid value.
    pub fn build(self) -> Result<ValidatedConfig, ConfigError> {
        if self.concurrency_limit == 0 {
            return Err(ConfigError::ConcurrencyLimitZero);
        }
        if self.concurrency_limit > tokio::sync::Semaphore::MAX_PERMITS {
            return Err(ConfigError::ConcurrencyLimitOverflow);
        }
        if self.ws_max_connections == 0 {
            return Err(ConfigError::WsMaxConnectionsZero);
        }
        if self.ws_max_connections > tokio::sync::Semaphore::MAX_PERMITS {
            return Err(ConfigError::WsMaxConnectionsOverflow);
        }
        if self.max_request_body_bytes < 1 {
            return Err(ConfigError::MaxRequestBodyBytesZero);
        }

        if let Some(ref key) = self.error_page_key {
            if key.is_empty() {
                return Err(ConfigError::ErrorPageKeyEmpty);
            }
            if key.starts_with('/') {
                return Err(ConfigError::ErrorPageKeyLeadingSlash);
            }
            if key.contains("..") {
                return Err(ConfigError::ErrorPageKeyTraversal);
            }
            if key.contains('\0') {
                return Err(ConfigError::ErrorPageKeyNullByte);
            }
            if key.contains('\\') {
                return Err(ConfigError::ErrorPageKeyBackslash);
            }
        }

        if let Some(ref csp) = self.csp_override {
            if !csp.is_ascii() {
                return Err(ConfigError::CspNotAscii);
            }
            if csp.contains('\r') || csp.contains('\n') {
                return Err(ConfigError::CspContainsCrlf);
            }
        }

        Ok(ValidatedConfig {
            concurrency_limit: self.concurrency_limit,
            max_request_body_bytes: self.max_request_body_bytes,
            ws_max_connections: self.ws_max_connections,
            csp_override: self.csp_override,
            error_page_key: self.error_page_key,
        })
    }
}

/// Validated, immutable server configuration.
///
/// Cannot be constructed directly — only via
/// [`ServerConfigBuilder::build()`]. All fields are private with
/// read-only accessor methods.
///
/// Not `Clone` by design: prevents extracting inner values to
/// construct a config that bypasses validation.
#[derive(Debug, PartialEq)]
#[non_exhaustive]
pub struct ValidatedConfig {
    concurrency_limit: usize,
    max_request_body_bytes: usize,
    ws_max_connections: usize,
    csp_override: Option<String>,
    error_page_key: Option<String>,
}

impl ValidatedConfig {
    /// Maximum concurrent in-flight HTTP requests.
    #[must_use]
    pub fn concurrency_limit(&self) -> usize {
        self.concurrency_limit
    }

    /// Maximum request body size in bytes.
    #[must_use]
    pub fn max_request_body_bytes(&self) -> usize {
        self.max_request_body_bytes
    }

    /// Maximum concurrent WebSocket connections.
    #[must_use]
    pub fn ws_max_connections(&self) -> usize {
        self.ws_max_connections
    }

    /// CSP override, if set.
    #[must_use]
    pub fn csp_override(&self) -> Option<&str> {
        self.csp_override.as_deref()
    }

    /// Error page cache key, if set.
    #[must_use]
    pub fn error_page_key(&self) -> Option<&str> {
        self.error_page_key.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_defaults_match_expected_values() {
        let config = ServerConfig::builder().build().unwrap();
        assert_eq!(config.concurrency_limit(), 1024);
        assert_eq!(config.max_request_body_bytes(), 1024);
        assert_eq!(config.ws_max_connections(), 200);
        assert!(config.csp_override().is_none());
        assert!(config.error_page_key().is_none());
    }

    #[test]
    fn builder_default_produces_ok() {
        assert!(ServerConfig::builder().build().is_ok());
    }

    #[test]
    fn builder_sets_concurrency_limit() {
        let config = ServerConfig::builder()
            .concurrency_limit(2048)
            .build()
            .unwrap();
        assert_eq!(config.concurrency_limit(), 2048);
    }

    #[test]
    fn builder_sets_max_request_body_bytes() {
        let config = ServerConfig::builder()
            .max_request_body_bytes(4096)
            .build()
            .unwrap();
        assert_eq!(config.max_request_body_bytes(), 4096);
    }

    #[test]
    fn builder_sets_ws_max_connections() {
        let config = ServerConfig::builder()
            .ws_max_connections(50)
            .build()
            .unwrap();
        assert_eq!(config.ws_max_connections(), 50);
    }

    #[test]
    fn builder_sets_csp_override() {
        let config = ServerConfig::builder()
            .csp_override("default-src 'self'")
            .build()
            .unwrap();
        assert_eq!(config.csp_override(), Some("default-src 'self'"));
    }

    #[test]
    fn builder_sets_error_page_key() {
        let config = ServerConfig::builder()
            .error_page_key("404.html")
            .build()
            .unwrap();
        assert_eq!(config.error_page_key(), Some("404.html"));
    }

    #[test]
    fn rejects_zero_concurrency_limit() {
        let err = ServerConfig::builder()
            .concurrency_limit(0)
            .build()
            .unwrap_err();
        assert_eq!(err, ConfigError::ConcurrencyLimitZero);
    }

    #[test]
    fn accepts_minimum_concurrency_limit() {
        let config = ServerConfig::builder()
            .concurrency_limit(1)
            .build()
            .unwrap();
        assert_eq!(config.concurrency_limit(), 1);
    }

    #[test]
    fn accepts_maximum_concurrency_limit() {
        let config = ServerConfig::builder()
            .concurrency_limit(tokio::sync::Semaphore::MAX_PERMITS)
            .build()
            .unwrap();
        assert_eq!(
            config.concurrency_limit(),
            tokio::sync::Semaphore::MAX_PERMITS
        );
    }

    #[test]
    fn rejects_overflow_concurrency_limit() {
        let err = ServerConfig::builder()
            .concurrency_limit(tokio::sync::Semaphore::MAX_PERMITS + 1)
            .build()
            .unwrap_err();
        assert_eq!(err, ConfigError::ConcurrencyLimitOverflow);
    }

    #[test]
    fn rejects_zero_ws_max_connections() {
        let err = ServerConfig::builder()
            .ws_max_connections(0)
            .build()
            .unwrap_err();
        assert_eq!(err, ConfigError::WsMaxConnectionsZero);
    }

    #[test]
    fn accepts_minimum_ws_max_connections() {
        let config = ServerConfig::builder()
            .ws_max_connections(1)
            .build()
            .unwrap();
        assert_eq!(config.ws_max_connections(), 1);
    }

    #[test]
    fn accepts_maximum_ws_max_connections() {
        let config = ServerConfig::builder()
            .ws_max_connections(tokio::sync::Semaphore::MAX_PERMITS)
            .build()
            .unwrap();
        assert_eq!(
            config.ws_max_connections(),
            tokio::sync::Semaphore::MAX_PERMITS
        );
    }

    #[test]
    fn rejects_overflow_ws_max_connections() {
        let err = ServerConfig::builder()
            .ws_max_connections(tokio::sync::Semaphore::MAX_PERMITS + 1)
            .build()
            .unwrap_err();
        assert_eq!(err, ConfigError::WsMaxConnectionsOverflow);
    }

    #[test]
    fn rejects_zero_body_limit() {
        let err = ServerConfig::builder()
            .max_request_body_bytes(0)
            .build()
            .unwrap_err();
        assert_eq!(err, ConfigError::MaxRequestBodyBytesZero);
    }

    #[test]
    fn accepts_minimum_body_limit() {
        let config = ServerConfig::builder()
            .max_request_body_bytes(1)
            .build()
            .unwrap();
        assert_eq!(config.max_request_body_bytes(), 1);
    }

    #[test]
    fn accepts_valid_error_page_key() {
        assert!(
            ServerConfig::builder()
                .error_page_key("404.html")
                .build()
                .is_ok()
        );
    }

    #[test]
    fn rejects_empty_error_page_key() {
        let err = ServerConfig::builder()
            .error_page_key("")
            .build()
            .unwrap_err();
        assert_eq!(err, ConfigError::ErrorPageKeyEmpty);
    }

    #[test]
    fn rejects_error_page_key_leading_slash() {
        let err = ServerConfig::builder()
            .error_page_key("/404.html")
            .build()
            .unwrap_err();
        assert_eq!(err, ConfigError::ErrorPageKeyLeadingSlash);
    }

    #[test]
    fn rejects_error_page_key_traversal() {
        let err = ServerConfig::builder()
            .error_page_key("../secret.html")
            .build()
            .unwrap_err();
        assert_eq!(err, ConfigError::ErrorPageKeyTraversal);
    }

    #[test]
    fn rejects_error_page_key_null_byte() {
        let err = ServerConfig::builder()
            .error_page_key("404\0.html")
            .build()
            .unwrap_err();
        assert_eq!(err, ConfigError::ErrorPageKeyNullByte);
    }

    #[test]
    fn rejects_error_page_key_backslash() {
        let err = ServerConfig::builder()
            .error_page_key("foo\\bar.html")
            .build()
            .unwrap_err();
        assert_eq!(err, ConfigError::ErrorPageKeyBackslash);
    }

    #[test]
    fn rejects_non_ascii_csp() {
        let err = ServerConfig::builder()
            .csp_override("default-src 'self' 🚀")
            .build()
            .unwrap_err();
        assert_eq!(err, ConfigError::CspNotAscii);
    }

    #[test]
    fn rejects_csp_with_cr() {
        let err = ServerConfig::builder()
            .csp_override("default-src 'self'\r")
            .build()
            .unwrap_err();
        assert_eq!(err, ConfigError::CspContainsCrlf);
    }

    #[test]
    fn rejects_csp_with_lf() {
        let err = ServerConfig::builder()
            .csp_override("default-src 'self'\n")
            .build()
            .unwrap_err();
        assert_eq!(err, ConfigError::CspContainsCrlf);
    }

    #[test]
    fn rejects_csp_with_crlf() {
        let err = ServerConfig::builder()
            .csp_override("default-src 'self'\r\nscript-src 'none'")
            .build()
            .unwrap_err();
        assert_eq!(err, ConfigError::CspContainsCrlf);
    }

    #[test]
    fn accepts_valid_ascii_csp() {
        assert!(
            ServerConfig::builder()
                .csp_override("default-src 'self'")
                .build()
                .is_ok()
        );
    }

    #[test]
    fn validated_config_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ValidatedConfig>();
    }

    #[test]
    fn config_error_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ConfigError>();
    }

    #[test]
    fn config_error_display_matches_expected() {
        assert_eq!(
            ConfigError::ConcurrencyLimitZero.to_string(),
            "concurrency_limit must be >= 1"
        );
        assert_eq!(
            ConfigError::CspContainsCrlf.to_string(),
            "csp_override must not contain CR or LF characters"
        );
    }
}
