#![forbid(unsafe_code)]
//!Compile-fail-only crate. Asserts that on default features the raw mint entry points for substrate-owned id types are not part of the public pardosa API. Lives in its own crate so it cannot accidentally inherit the `test-support` feature activation through pardosa's dev-dependency graph (mission pardosa-architecture-spine-20260526 track spine-02).
