# GEN-0033. GenomeOrd Marker Trait for Map Keys

Date: 2026-04-25
Last-reviewed: 2026-04-25
Tier: A
Status: Accepted

## Related

References: GEN-0032

## Context

`BTreeMap<K, V>` serializes entries in `Ord` order, which is deterministic for
standard library types (`u32`, `String`, etc.). However, the `GenomeSafe` trait
alone does not enforce that map key types have a deterministic `Ord` implementation.
Any type implementing `GenomeSafe + Ord` could be used as a map key, including types
with non-deterministic ordering (e.g., ordering dependent on locale, environment
variables, or runtime state).

A compile-time mechanism is needed to restrict map keys to types with known-good
ordering properties.

## Decision

Introduce `GenomeOrd`, a marker trait that asserts a type has a deterministic, total,
platform-independent `Ord` implementation suitable for `BTreeMap`/`BTreeSet` keys:

```rust
pub trait GenomeOrd: GenomeSafe {}
```

**Implementations provided (owned value types only):**
- Primitives: `bool`, `u8`–`u128`, `i8`–`i128`, `char`, `()`
- Strings: `String`
- Composites: `Option<T: GenomeOrd>`, `[T: GenomeOrd; N]`, tuples (1–16 elements)

**Explicitly excluded:**
- `f32`, `f64` — do not implement `Ord` in std (`PartialOrd` only)
- `Box<T>`, `Arc<T>`, `Cow<'_, T>` — runtime/memory wrappers, not data types;
  use the owned equivalent (e.g., `String` not `Box<str>`)
- `&str`, `&[u8]` — borrowed types; not idiomatic as owned map keys
- `Vec<T>` — variable-length containers are not idiomatic as map keys
- `PhantomData<T>` — zero-size, not meaningful as a map key

**BTreeMap/BTreeSet bounds updated:**
```rust
impl<K: GenomeSafe + GenomeOrd, V: GenomeSafe> GenomeSafe for BTreeMap<K, V> { ... }
impl<T: GenomeSafe + GenomeOrd> GenomeSafe for BTreeSet<T> { ... }
```

**Derive macro integration:**
The `#[derive(GenomeSafe)]` macro recursively walks field types to detect generic
parameters used in `BTreeMap` key or `BTreeSet` element position. These parameters
automatically receive `GenomeOrd` bounds in the generated impl. For example:

```rust
#[derive(GenomeSafe)]
struct Indexed<K> {
    entries: BTreeMap<K, u32>,
}
// Generates: impl<K: GenomeSafe + GenomeOrd> GenomeSafe for Indexed<K> { ... }
```

**Custom key types:**
Users implement `GenomeOrd` manually (no derive macro):
```rust
#[derive(PartialEq, Eq, PartialOrd, Ord, GenomeSafe)]
struct MyKey { id: u64 }
impl GenomeOrd for MyKey {}
```

### Trust boundary

`GenomeOrd` is a **safe** trait. The compiler cannot verify that an `Ord`
implementation is truly deterministic and total. A type could implement `GenomeOrd`
while having a pathological `Ord` that depends on thread-local state, producing
non-canonical serialization.

Defense-in-depth: `verify_roundtrip` (serialize → deserialize → re-serialize → compare)
catches ordering violations at runtime.

### Known limitations

- **Type aliases:** The derive macro detects `BTreeMap`/`BTreeSet` by last path
  segment name. Type aliases (e.g., `type MyMap<K, V> = BTreeMap<K, V>`) are not
  detected. Users must add `GenomeOrd` bounds manually in such cases.
- **Breaking change:** Adding `GenomeOrd` to `BTreeMap`/`BTreeSet` bounds is a
  compile-breaking change for existing custom key types. Migration: add
   `impl GenomeOrd for MyKey {}`. Acceptable pre-1.0.

R1 [4]: GenomeOrd is a marker trait asserting deterministic, total,
  platform-independent Ord suitable for BTreeMap and BTreeSet keys
R2 [4]: BTreeMap and BTreeSet require K: GenomeSafe + GenomeOrd
  enforced at compile time
R3 [4]: The derive macro auto-detects generic parameters used in
  BTreeMap key position and adds GenomeOrd bounds automatically

## Consequences

- **Positive:** Compile-time enforcement that map keys have deterministic ordering.
  Invalid key types produce clear `GenomeOrd is not satisfied` errors.
- **Positive:** Derive macro auto-detects generic key parameters — no manual
  annotation needed for the common case.
- **Positive:** Opinionated impl set (owned value types only) prevents accidental
  use of smart pointers as map keys.
- **Negative:** Custom key types require a manual `impl GenomeOrd for T {}` line.
- **Negative:** Type alias limitation may produce confusing errors in edge cases.
