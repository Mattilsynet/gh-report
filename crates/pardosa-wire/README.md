# pardosa-wire

In-house canonical encoding format for pardosa events (GEN-0035). `pardosa-wire`
is the substrate-purity layer: a `#![no_std] + extern crate alloc;` codec that
turns event types into stable byte sequences and back, with no payload-typed
vocabulary baked in.

Part of the [pardosa](https://github.com/acje/rescue-pardosa) workspace.

## Overview

The crate exposes two paired traits, `Encode` and `Decode`, along with
free-function helpers `to_vec`, `from_bytes`, and `from_bytes_with_cap`. Every
decode path is bounded: `from_bytes` applies the `DEFAULT_DECODE_CAP` cap so
adversarial inputs cannot force unbounded allocation, and
`from_bytes_with_cap` lets callers set a tighter cap for high-trust paths.
This is GEN-0035 cap discipline — the substrate refuses to grow without a
cap chosen at the call site.

`Validate` and `Timestamp` are domain-shape contracts that travel with the
codec — a value that round-trips through `to_vec` / `from_bytes` must satisfy
its own `Validate` impl on the way out, not just on the way in. The
`EventSafe` marker, together with the `sealed` module, gates which types may
participate in pardosa events at all; downstream crates (`pardosa-schema`,
`pardosa-derive`) extend this sealed graph rather than opening it.

The crate is dependency-free in its default surface. Foreign-payload support
(`uuid`, `bytes`, `arrayvec`, `jiff`) lives behind off-by-default Cargo features
per GEN-0041 so the no-features build stays minimal. The `blake3` feature
enables `precursor_hash_of` for the PAR-0021 precursor-hash helper.

## Example

```rust,no_run
use pardosa_wire::{from_bytes, to_vec};

let bytes = to_vec(&42u32);
let back: u32 = from_bytes(&bytes).expect("decode");
assert_eq!(back, 42);
```

## License

Licensed under either of

- Apache License, Version 2.0
- MIT License

at your option.
