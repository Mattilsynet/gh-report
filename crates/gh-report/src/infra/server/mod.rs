//! In-memory HTTP serve pipeline shim.
//!
//! The implementation lives in `cherry-pit-web`; this module preserves
//! existing `gh-report` paths while app-side state implements the public
//! trait from the library.

pub use cherry_pit_web::serve::{config, error, runtime, state};
