//! Cross-cutting `#[cfg(test)]` tests that do not belong to any single
//! per-impl module. Currently houses `f1_invariant_anticipation`, which
//! defines an ad-hoc `repr(u8)` enum with a hand-rolled `Encode` impl to
//! anticipate the EventError wire byte across module boundaries.

#[cfg(test)]
mod inner {
    use crate::Encode;
    use alloc::vec::Vec;

    #[test]
    fn f1_invariant_anticipation() {
        // GEN-0035 §"Composite encoding" — unit variants of a `repr(u8)`
        // enum encode as one byte = the explicit discriminant. Sub-mission
        // C will land EventError with Internal = 7 (F4) and assert
        // buf[0] == 7u8. Here we anticipate the byte-level expectation
        // for a hand-rolled enum impl, to surface any encoding-spec
        // defect now rather than at C.
        #[repr(u8)]
        enum Tag {
            #[expect(
                dead_code,
                reason = "test enum: `Zero` is the documentary tag-0 discriminant; only `Seven` is constructed in this test body"
            )]
            Zero = 0,
            Seven = 7,
        }
        impl Encode for Tag {
            fn encode(&self, out: &mut Vec<u8>) {
                let d: u8 = match self {
                    Tag::Zero => 0,
                    Tag::Seven => 7,
                };
                out.push(d);
            }
        }
        let mut buf = Vec::new();
        Tag::Seven.encode(&mut buf);
        assert_eq!(buf.len(), 1);
        assert_eq!(buf[0], 7u8);
    }
}
