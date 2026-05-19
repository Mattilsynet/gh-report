//! GraphQL stub. M1.4 replaces this with the real
//! `async_graphql::Schema<Query, EmptyMutation, EmptySubscription>`
//! and the resolvers that read from `AppState`'s projection cache.

/// Placeholder schema constructor. Calling site (`AppState`)
/// constructs the real schema in M1.4. Returns `()` to keep the
/// surface minimal while still being threadable through `AppState`.
pub const fn schema_stub() {}
