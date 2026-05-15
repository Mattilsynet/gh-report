//! Regression: CHE-0055 §G3+G4 MC-2.5 — [`cherry_pit_web::ServerError`]
//! is publicly nameable from outside cherry-pit-web (feature = "projection").

#![cfg(feature = "projection")]

#[test]
fn server_error_resolves_at_crate_root() {
    // Compile-time assertion: the type is reachable at the documented
    // public path. Constructing a value exercises the variant fields'
    // public accessibility. MC-2.5; CHE-0055 §G3+G4.
    let _err: cherry_pit_web::ServerError = cherry_pit_web::ServerError::InvalidAddress {
        address: String::from("not-an-address"),
        source: "x".parse::<std::net::SocketAddr>().unwrap_err(),
    };
}
