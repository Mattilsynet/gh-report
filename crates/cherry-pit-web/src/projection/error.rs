//! Projection server lifecycle errors.
//!
//! Ported from the donor crate's `error` module per m5 Phase 3b. Distinct from the
//! CHE-0049 R10 HTTP response envelope (`crate::middleware::error`):
//! these errors surface at server-bootstrap and runtime, not in HTTP
//! response bodies.
//!
//! Public per CHE-0055 §G3+G4 (MC-2.5). Gated behind
//! `feature = "projection"` and re-exported from the crate root
//! (`cherry_pit_web::ServerError`).

use thiserror::Error;

/// Errors related to the projection web server lifecycle.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ServerError {
    /// The bind address string could not be parsed as a valid socket address.
    #[error("invalid bind address '{address}': {source}")]
    InvalidAddress {
        /// The malformed address as supplied.
        address: String,
        /// Underlying parse error.
        source: std::net::AddrParseError,
    },

    /// The server could not bind to the requested address/port.
    #[error("server bind failed on {address}")]
    BindFailed {
        /// The address the server attempted to bind.
        address: std::net::SocketAddr,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// A runtime error occurred while serving requests.
    #[error("server runtime error")]
    RuntimeFailed(#[source] std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_address_display_includes_address_and_source() {
        let err: Result<std::net::SocketAddr, _> = "not-an-address".parse();
        let server_err = ServerError::InvalidAddress {
            address: "not-an-address".into(),
            source: err.unwrap_err(),
        };
        let msg = server_err.to_string();
        assert!(msg.contains("not-an-address"), "should contain address");
        assert!(msg.contains("invalid"), "should contain parse error");
    }

    #[test]
    fn bind_failed_exposes_source_error() {
        use std::error::Error;

        let io_err = std::io::Error::new(std::io::ErrorKind::AddrInUse, "port taken");
        let server_err = ServerError::BindFailed {
            address: "127.0.0.1:8080".parse().unwrap(),
            source: io_err,
        };
        assert!(server_err.source().is_some(), "should have source error");
    }

    #[test]
    fn runtime_failed_exposes_source_error() {
        use std::error::Error;

        let io_err = std::io::Error::other("runtime failure");
        let server_err = ServerError::RuntimeFailed(io_err);
        assert!(server_err.source().is_some(), "should have source error");
    }
}
