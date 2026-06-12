use crate::{EventSafe, sealed};
macro_rules! seal_primitive {
    ($($ty:ty),+ $(,)?) => {
        $(impl sealed::Sealed for $ty {} impl EventSafe for $ty {})+
    };
}
seal_primitive!(bool, u8, u16, u32, u64, u128, i8, i16, i32, i64, i128, char);
impl<T: EventSafe> sealed::Sealed for Option<T> {}
impl<T: EventSafe> EventSafe for Option<T> {}
impl<T: EventSafe, const N: usize> sealed::Sealed for [T; N] {}
impl<T: EventSafe, const N: usize> EventSafe for [T; N] {}
macro_rules! seal_tuple {
    ($($T:ident),+) => {
        impl <$($T : EventSafe),+> sealed::Sealed for ($($T,)+) {} impl <$($T :
        EventSafe),+> EventSafe for ($($T,)+) {}
    };
}
seal_tuple!(T0);
seal_tuple!(T0, T1);
seal_tuple!(T0, T1, T2);
seal_tuple!(T0, T1, T2, T3);
seal_tuple!(T0, T1, T2, T3, T4);
seal_tuple!(T0, T1, T2, T3, T4, T5);
seal_tuple!(T0, T1, T2, T3, T4, T5, T6);
seal_tuple!(T0, T1, T2, T3, T4, T5, T6, T7);
seal_tuple!(T0, T1, T2, T3, T4, T5, T6, T7, T8);
seal_tuple!(T0, T1, T2, T3, T4, T5, T6, T7, T8, T9);
seal_tuple!(T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10);
seal_tuple!(T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11);
seal_tuple!(T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12);
seal_tuple!(T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13);
seal_tuple!(
    T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14
);
seal_tuple!(
    T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14, T15
);
#[cfg(feature = "std")]
mod std_blankets {
    use crate::{EventSafe, sealed};
    use alloc::boxed::Box;
    use alloc::sync::Arc;
    impl<T: EventSafe> sealed::Sealed for Box<T> {}
    impl<T: EventSafe> EventSafe for Box<T> {}
    impl<T: EventSafe> sealed::Sealed for Arc<T> {}
    impl<T: EventSafe> EventSafe for Arc<T> {}
}
