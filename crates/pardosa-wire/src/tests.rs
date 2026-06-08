#[cfg(test)]
mod inner {
    use crate::Encode;
    use alloc::vec::Vec;
    #[test]
    fn f1_invariant_anticipation() {
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
