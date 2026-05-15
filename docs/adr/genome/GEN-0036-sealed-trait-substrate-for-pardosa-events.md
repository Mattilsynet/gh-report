# GEN-0036. Sealed Trait Substrate for Pardosa Events

Date: 2026-05-15
Last-reviewed: 2026-05-15
Tier: A
Status: Accepted

## Related

References: GEN-0033, GEN-0035, PAR-0024

## Context

`GenomeSafe` gates deterministic serialization. The v2-r2 typing refresh
adds a new root marker `EventSafe` and seals the stack so external crates
cannot widen the safe-type set. Sealing makes "blessed-by-derive" a
compile-time property.

A new `pardosa-traits` crate hosts `EventSafe` and `Sealed`, separate from
`pardosa-genome`. Two A2.1 tensions are recorded here:

1. **Encode supertrait deferred (F2).** Original A2 wired
   `EventSafe: Encode + Sealed`. `Encode` derive emission is A2.2, so the
   supertrait would non-mechanically break trybuild fixtures (`Point`,
   `Container`) that derive `GenomeSafe` without `Encode`. A2.1 ships
   `EventSafe: Sealed`. A2.2 strengthens to `EventSafe: Encode + Sealed`
   atomically with derive emission; see GEN-0037.

2. **Orphan rule (R-B).** E0117 / E0210 forbids impl'ing a foreign trait
   for a foreign type from a third crate. Trusted `Sealed + EventSafe`
   blankets for std types (`Box<T>`, `Vec<T>`, `BTreeMap<K, V>`, …)
   therefore live in `pardosa-traits`, not `pardosa-genome`. The original
   `#![no_std]` goal for `pardosa-traits` was implicit, not load-bearing;
   relaxing to default-std with zero external deps preserves the
   substrate-crate split.

## Decision

Introduce a `pardosa-traits` workspace crate hosting the sealed trait
substrate. The trait stack from root to leaf is:

```text
sealed::Sealed  ←  EventSafe  ←  GenomeSafe  ←  GenomeOrd
```

```rust
// crates/pardosa-traits/src/lib.rs
pub mod sealed { pub trait Sealed {} }
pub trait EventSafe: sealed::Sealed {}
```

```rust
// crates/pardosa-genome/src/genome_safe.rs
pub trait GenomeSafe: pardosa_traits::EventSafe {
    const SCHEMA_HASH: u128;
    const SCHEMA_SOURCE: &'static str;
}
pub trait GenomeOrd: GenomeSafe {}
```

**Crate boundaries (trait-with-its-blankets):**

| Crate | Owns trait | Owns trusted blankets for |
|---|---|---|
| `pardosa-traits` | `Sealed`, `EventSafe` | primitives, `()`, `str`, `String`, `&str`, `&[u8]`, `Option<T>`, `Vec<T>`, `Box<T>`, `BTreeMap<K, V>`, `BTreeSet<T>`, `Arc<T>`, `Cow<'_, T>`, `PhantomData<T>`, `[T; N]`, tuples 1..=16 |
| `pardosa-genome` | `GenomeSafe`, `GenomeOrd` | the same set, with hash + source data |
| `pardosa-encoding` | `Encode`, `Decode` (per GEN-0035) | std type blankets (B sub-mission) |

`pardosa-derive` emits `Sealed + EventSafe + GenomeSafe` impls atomically
for every `#[derive(GenomeSafe)]` user type.

`pardosa-genome` re-exports `pardosa_traits::{EventSafe, sealed}` so
consumers can `use pardosa_genome::EventSafe` without depending on
`pardosa-traits` directly.

### Sealing mechanism

`sealed::Sealed` lives in a `pub mod sealed` whose only impls are the
trusted blankets above plus the derive macro. External crates can name
`pardosa_traits::sealed::Sealed` but cannot impl it for their own types
without coherence violation — the orphan rule that forced R-B is the same
rule that makes the sealing strong. A trybuild compile-fail fixture
(`tests/compile_fail/sealed_eventsafe_external_impl.rs`) falsifies any
attempt to impl `EventSafe` for an outsider type.

### Forward note (A2.2 / GEN-0037)

A2.2 will (i) extend `pardosa-derive` to emit `Encode`/`Decode` for user
types and (ii) atomically add the `Encode` supertrait, making the stack
`EventSafe: Encode + sealed::Sealed`. This closes the F2 deferral. The
crate-boundary table above already anticipates the B sub-mission: `Encode`
std blankets live in `pardosa-encoding` (with the `Encode` trait), not in
`pardosa-traits`. Trait-with-its-blankets co-location holds for both.

R1 [4]: `EventSafe` is the root marker of the pardosa event-type stack;
  `GenomeSafe: EventSafe` and `GenomeOrd: GenomeSafe`
R2 [4]: `EventSafe: sealed::Sealed` makes `EventSafe` un-impl'able from
  external crates without a derive blessing
R3 [4]: Trusted blanket `Sealed + EventSafe` impls for std types live in
  `pardosa-traits` to satisfy the Rust orphan rule
R4 [4]: `#[derive(GenomeSafe)]` emits `Sealed + EventSafe + GenomeSafe`
  impls atomically for the annotated type

## Consequences

- **Positive:** Compile-time enforcement that only blessed types implement
  the marker stack. External crates get a clear `Sealed is not implemented`
  error on unsanctioned impls.
- **Positive:** Substrate-crate split survives R-B; `pardosa-traits` is a
  stable dependency target for B's std `EventSafe` blankets and F's
  wrapper types.
- **Positive:** A2.1 verify gate green without touching existing trybuild
  fixtures — F2 avoids a non-mechanical break on `Point` / `Container`.
- **Negative:** `pardosa-traits` is not `#![no_std]`. Revisit only if a
  no-std embedded target becomes a real requirement.
- **Negative:** `EventSafe` is a pure marker in this revision; the full
  guarantee (Encode-able + sealed) only lands at A2.2. Read GEN-0036 +
  GEN-0037 as a pair for the post-A2.2 contract.
