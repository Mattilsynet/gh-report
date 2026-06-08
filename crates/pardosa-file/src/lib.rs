#![forbid(unsafe_code)]
/// Workspace auto-trait policy macro (mission rescue-pardosa-59y0).
///
/// Uses **stable built-in `Send`/`Sync` bounds** only — no custom
/// `auto trait`, no `#![feature(auto_traits)]`, no `negative_impls`.
/// See pardosa-schema/src/lib.rs for the full doctrine.
macro_rules! assert_auto_traits {
    (
        $(SendSync { $($ss:ty),* $(,)? })? $(SendOnly { $($so:ty),* $(,)? })? $(NotSend {
        $($ns:ty),* $(,)? })?
    ) => {
        const _ : fn () = || { fn __assert_send_sync < T : Send + Sync > () {} fn
        __assert_send < T : Send > () {} $($(__assert_send_sync::<$ss > ();)*)?
        $($(__assert_send::<$so > ();)*)? $($(let _ = ::core::marker::PhantomData::<$ns
        >;)*)? };
    };
}
mod append;
mod config;
mod error;
pub mod format;
pub mod manifest;
mod options;
mod reader;
mod syncable;
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub mod test_support;
mod writer;
pub use append::AppendWriter;
pub use config::PageClass;
pub use error::FileError;
pub use options::{Compression, ReaderOptions, WriterOptions};
pub use reader::{IndexEntry, MessageIter, Reader};
pub use syncable::{Syncable, fsync_parent_dir};
pub use writer::Writer;
// AUTO-TRAIT-POLICY-BEGIN
assert_auto_traits! {
    SendSync { PageClass, FileError, Compression, WriterOptions, ReaderOptions,
    IndexEntry, Reader < std::io::Cursor < std::vec::Vec < u8 >>>, MessageIter <'static,
    std::io::Cursor < std::vec::Vec < u8 >>>, Writer <'static, std::io::Cursor <
    std::vec::Vec < u8 >>>, AppendWriter <'static, std::io::Cursor < std::vec::Vec < u8
    >>>, manifest::ManifestRecord, manifest::ManifestSnapshot, manifest::RecoveredPrefix,
    manifest::RecoveryError, } SendOnly {} NotSend {}
}
// AUTO-TRAIT-POLICY-END
