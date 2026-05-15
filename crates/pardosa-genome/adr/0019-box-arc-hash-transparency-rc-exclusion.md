# ADR-019: Box and Arc hash transparency with Rc exclusion

**Status:** Accepted

### Context

Rust's smart pointer types (`Box<T>`, `Arc<T>`, `Rc<T>`) are wrappers that do
not affect serialization — serde serializes the inner value identically regardless
of the wrapper. The schema hash must decide whether wrapping or unwrapping a smart
pointer is a schema-compatible change.

A separate concern is which smart pointers should be supported at all. `Rc<T>` is
`!Send`, making it incompatible with async runtimes (Tokio, Axum) — the primary
deployment context for pardosa services.

### Decision

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
documented as hash-transparent in genome.md but require a `?Sized` bound
adjustment to compile. This will be addressed in a follow-up change.

### Consequences

- **Positive:** Refactoring between `T`, `Box<T>`, and `Arc<T>` is
  schema-compatible. Common Rust refactoring pattern preserved.
- **Positive:** Forcing `Arc` over `Rc` aligns with async-first architecture.
  Prevents `!Send` types from entering serializable data models.
- **Negative:** `Rc` users must refactor to `Arc` before serialization. Deliberate
  friction — `Rc` in async contexts is a latent bug.
- **Negative:** `Box<str>` and `Arc<str>` do not currently compile as
  `GenomeSafe` due to missing `?Sized` bound. Documented for follow-up.

### References

- `genome_safe.rs` — `Box<T>` impl (line 159), `Arc<T>` impl (line 187),
  `Rc` exclusion comment (line 184)
- `tests/genome_safe.rs` — `box_hash_transparent`, `arc_hash_transparent`
- genome.md §Limitations (excluded wrapper types)
