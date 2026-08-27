//! Presentation options for the generic serve surface.
//!
//! Configuration flows through two types:
//!
//! - [`ServeOptionsBuilder`] — mutable builder with setter methods.
//!   Created via [`ServeOptions::builder()`].
//! - [`ServeOptions`] — immutable, validated options. Created only via
//!   [`ServeOptionsBuilder::build()`].
//!
//! This pattern makes invalid configuration unrepresentable:
//! [`super::runtime::build_router`] accepts `&ServeOptions`, so callers
//! cannot pass unchecked values.
//!
//! Sizing lives elsewhere. Per CHE-0062:R3 a number reachable through
//! two parameters is two sources of truth, so the body ceiling and
//! in-flight cap are carried by [`crate::LayerLimits`] and the WS cap
//! by [`crate::WsPolicy`]. What remains here is presentation only:
//! the CSP header value and the custom error-page cache key.

use thiserror::Error;

/// Typed errors for options validation.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum ConfigError {
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

/// Builder for [`ServeOptions`].
#[derive(Debug, Clone, Default)]
pub struct ServeOptionsBuilder {
    csp_override: Option<String>,
    error_page_key: Option<String>,
}

impl ServeOptionsBuilder {
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

    /// Validate and build the options.
    ///
    /// # Errors
    ///
    /// Returns `Err(ConfigError)` if any field has an invalid value.
    pub fn build(self) -> Result<ServeOptions, ConfigError> {
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

        Ok(ServeOptions {
            csp_override: self.csp_override,
            error_page_key: self.error_page_key,
        })
    }
}

/// Validated, immutable presentation options for the serve surface.
///
/// Cannot be constructed directly — only via
/// [`ServeOptionsBuilder::build()`]. All fields are private with
/// read-only accessor methods.
///
/// Not `Clone` by design: prevents extracting inner values to
/// construct options that bypass validation.
#[derive(Debug, PartialEq)]
#[non_exhaustive]
pub struct ServeOptions {
    csp_override: Option<String>,
    error_page_key: Option<String>,
}

impl ServeOptions {
    /// Create a new [`ServeOptionsBuilder`] with default values.
    ///
    /// Defaults: `csp_override` `None`, `error_page_key` `None`.
    #[must_use]
    pub fn builder() -> ServeOptionsBuilder {
        ServeOptionsBuilder::default()
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
        let options = ServeOptions::builder().build().unwrap();
        assert!(options.csp_override().is_none());
        assert!(options.error_page_key().is_none());
    }

    #[test]
    fn builder_default_produces_ok() {
        assert!(ServeOptions::builder().build().is_ok());
    }

    #[test]
    fn builder_sets_csp_override() {
        let options = ServeOptions::builder()
            .csp_override("default-src 'self'")
            .build()
            .unwrap();
        assert_eq!(options.csp_override(), Some("default-src 'self'"));
    }

    #[test]
    fn builder_sets_error_page_key() {
        let options = ServeOptions::builder()
            .error_page_key("404.html")
            .build()
            .unwrap();
        assert_eq!(options.error_page_key(), Some("404.html"));
    }

    #[test]
    fn accepts_valid_error_page_key() {
        assert!(
            ServeOptions::builder()
                .error_page_key("404.html")
                .build()
                .is_ok()
        );
    }

    #[test]
    fn rejects_empty_error_page_key() {
        let err = ServeOptions::builder()
            .error_page_key("")
            .build()
            .unwrap_err();
        assert_eq!(err, ConfigError::ErrorPageKeyEmpty);
    }

    #[test]
    fn rejects_error_page_key_leading_slash() {
        let err = ServeOptions::builder()
            .error_page_key("/404.html")
            .build()
            .unwrap_err();
        assert_eq!(err, ConfigError::ErrorPageKeyLeadingSlash);
    }

    #[test]
    fn rejects_error_page_key_traversal() {
        let err = ServeOptions::builder()
            .error_page_key("../secret.html")
            .build()
            .unwrap_err();
        assert_eq!(err, ConfigError::ErrorPageKeyTraversal);
    }

    #[test]
    fn rejects_error_page_key_null_byte() {
        let err = ServeOptions::builder()
            .error_page_key("404\0.html")
            .build()
            .unwrap_err();
        assert_eq!(err, ConfigError::ErrorPageKeyNullByte);
    }

    #[test]
    fn rejects_error_page_key_backslash() {
        let err = ServeOptions::builder()
            .error_page_key("foo\\bar.html")
            .build()
            .unwrap_err();
        assert_eq!(err, ConfigError::ErrorPageKeyBackslash);
    }

    #[test]
    fn rejects_non_ascii_csp() {
        let err = ServeOptions::builder()
            .csp_override("default-src 'self' 🚀")
            .build()
            .unwrap_err();
        assert_eq!(err, ConfigError::CspNotAscii);
    }

    #[test]
    fn rejects_csp_with_cr() {
        let err = ServeOptions::builder()
            .csp_override("default-src 'self'\r")
            .build()
            .unwrap_err();
        assert_eq!(err, ConfigError::CspContainsCrlf);
    }

    #[test]
    fn rejects_csp_with_lf() {
        let err = ServeOptions::builder()
            .csp_override("default-src 'self'\n")
            .build()
            .unwrap_err();
        assert_eq!(err, ConfigError::CspContainsCrlf);
    }

    #[test]
    fn rejects_csp_with_crlf() {
        let err = ServeOptions::builder()
            .csp_override("default-src 'self'\r\nscript-src 'none'")
            .build()
            .unwrap_err();
        assert_eq!(err, ConfigError::CspContainsCrlf);
    }

    #[test]
    fn accepts_valid_ascii_csp() {
        assert!(
            ServeOptions::builder()
                .csp_override("default-src 'self'")
                .build()
                .is_ok()
        );
    }

    #[test]
    fn serve_options_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ServeOptions>();
    }

    #[test]
    fn config_error_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ConfigError>();
    }

    #[test]
    fn config_error_display_matches_expected() {
        assert_eq!(
            ConfigError::ErrorPageKeyEmpty.to_string(),
            "error_page_key must not be empty"
        );
        assert_eq!(
            ConfigError::CspContainsCrlf.to_string(),
            "csp_override must not contain CR or LF characters"
        );
    }
}
