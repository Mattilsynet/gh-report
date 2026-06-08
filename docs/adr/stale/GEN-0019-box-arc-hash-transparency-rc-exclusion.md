# GEN-0019. Box and Arc Hash Transparency — Rc Exclusion

Date: 2026-04-25
Last-reviewed: 2026-04-28
Tier: D
Status: Accepted

## Related

References: GEN-0004

## Context

Rust's smart pointers (`Box<T>`, `Arc<T>`, `Rc<T>`) do not affect serde serialization — the inner value serializes identically. The schema hash must decide whether wrapping/unwrapping is schema-compatible. Separately, `Rc<T>` is `!Send`, incompatible with async runtimes — the primary deployment context.

## Decision

**Box and Arc are hash-transparent.** Both delegate to `T`'s `SCHEMA_HASH` and
`SCHEMA_SOURCE`. Wrapping or unwrapping `Box<T>` / `Arc<T>` is a
schema-compatible change — no migration required.

```rust
impl<T: GenomeSafe> GenomeSafe for Box<T> {
    const SCHEMA_HASH: u64 = T::SCHEMA_HASH;        // transparent
    const SCHEMA_SOURCE: &'static str = T::SCHEMA_SOURCE;
}

impl<T: GenomeSafe> GenomeSafe for std::sync::Arc<T> {
    const SCHEMA_HASH: u64 = T::SCHEMA_HASH;        // transparent
    const SCHEMA_SOURCE: &'static str = T::SCHEMA_SOURCE;
}
```

**Rc is excluded.** No `GenomeSafe` implementation exists for `Rc<T>`. Attempting
to derive `GenomeSafe` for a struct with an `Rc<T>` field produces a compile
error (`Rc<T>: GenomeSafe` is not satisfied). Users needing shared ownership
must use `Arc<T>`.

**Known limitation:** The current `Box<T>` and `Arc<T>` impls require `T: Sized`
(implicit bound). `Box<str>` and `Arc<str>` — where `T` is unsized — are
hash-transparent by design but require a
`?Sized` bound adjustment to compile. This will be addressed in a follow-up change.

R1 [9]: Box and Arc delegate to T's SCHEMA_HASH and SCHEMA_SOURCE
  making wrapping and unwrapping a schema-compatible change
R2 [9]: No GenomeSafe implementation exists for Rc — attempting to
  derive GenomeSafe with an Rc field produces a compile error
R3 [9]: Users needing shared ownership must use Arc instead of Rc

## Consequences

- Refactoring between `T`, `Box<T>`, and `Arc<T>` is schema-compatible.
- Forcing `Arc` over `Rc` aligns with async-first architecture, preventing `!Send` types in data models.
- `Rc` users must refactor to `Arc`. Deliberate friction — `Rc` in async is a latent bug.
- `Box<str>` and `Arc<str>` need `?Sized` bound adjustment. Documented for follow-up.
