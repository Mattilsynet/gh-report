//! Process-level TLS crypto provider installation.
//!
//! Both the `ring` and `aws-lc-rs` rustls `CryptoProvider`s are compiled
//! into this binary (`ring` via `async-nats`, `aws-lc-rs` via `reqwest`).
//! With more than one provider present, rustls 0.23 refuses to choose a
//! process default and panics on first TLS use. The MAP NATS `JetStream`
//! backend connects over `tls://`, so the default must be installed
//! explicitly before any TLS handshake.

use rustls::crypto::CryptoProvider;
use rustls::crypto::ring::default_provider;

/// Install `ring` as the process-wide rustls `CryptoProvider` if none is set.
///
/// Idempotent: a second call (or a provider installed elsewhere) leaves the
/// existing default untouched. Call once at process start, before any
/// component opens a TLS connection.
pub fn install_default_crypto_provider() {
    if CryptoProvider::get_default().is_none() {
        let _ = default_provider().install_default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_sets_a_process_default_provider() {
        install_default_crypto_provider();
        assert!(
            CryptoProvider::get_default().is_some(),
            "a rustls CryptoProvider must be installed after init"
        );
    }

    #[test]
    fn install_is_idempotent() {
        install_default_crypto_provider();
        install_default_crypto_provider();
        assert!(CryptoProvider::get_default().is_some());
    }
}
