//! Regression guard: [`EventStore::create`] fences the new `.pgno`
//! file's directory entry via [`pardosa_file::fsync_parent_dir`].
//!
//! Exercises the structural paths that would regress if the
//! `fsync_parent_dir` call site were removed: create in a freshly-made
//! subdirectory, create-drop-reopen round trip, and create with a bare
//! relative filename (parent unwraps to `.`).
use pardosa::store::{EventStore, GenomeSafe, HasEventSchemaSource};
use std::path::PathBuf;
#[derive(Debug, Clone, PartialEq, Eq, GenomeSafe)]
struct Payload {
    v: u64,
}
impl HasEventSchemaSource for Payload {
    const EVENT_SCHEMA_SOURCE: Option<&'static str> = None;
}
fn unique_subdir(tag: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system time")
        .as_nanos();
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let mut p = std::env::temp_dir();
    p.push(format!("pardosa-77m2-{tag}-{pid}-{nanos}-{seq}"));
    p
}
struct TmpDir(PathBuf);
impl TmpDir {
    fn new(tag: &str) -> Self {
        let p = unique_subdir(tag);
        std::fs::create_dir_all(&p).expect("create_dir_all");
        Self(p)
    }
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}
impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
#[test]
fn create_in_fresh_subdirectory_succeeds_and_reopens() {
    let dir = TmpDir::new("subdir");
    let pgno = dir.path().join("journal.pgno");
    {
        let mut store = EventStore::<Payload>::create(&pgno).expect("create in subdir");
        let mut w = store.writer();
        let _ = w.begin(Payload { v: 1 }).expect("begin");
        let _ = w.sync().expect("sync");
    }
    assert!(pgno.exists(), ".pgno file must persist after create+drop");
    let _ = EventStore::<Payload>::open(&pgno).expect("reopen after create");
}
#[test]
fn create_then_drop_leaves_pgno_file_on_disk() {
    let dir = TmpDir::new("rtmin");
    let pgno = dir.path().join("min.pgno");
    {
        let _ = EventStore::<Payload>::create(&pgno).expect("create");
    }
    assert!(
        pgno.exists(),
        ".pgno file must persist after create+drop (regression guard: \
         fsync_parent_dir call site must not fail on a bare create)"
    );
}
#[test]
fn create_with_bare_filename_relative_to_cwd_unwraps_parent_to_dot() {
    let dir = TmpDir::new("relparent");
    let cwd_guard = ChdirGuard::push(dir.path());
    let bare = std::path::Path::new("bare.pgno");
    {
        let _ = EventStore::<Payload>::create(bare).expect("create relative bare filename");
    }
    assert!(
        dir.path().join("bare.pgno").exists(),
        "bare-filename create must materialise in cwd"
    );
    drop(cwd_guard);
}
struct ChdirGuard {
    prior: PathBuf,
    _lock: std::sync::MutexGuard<'static, ()>,
}
impl ChdirGuard {
    fn push(p: &std::path::Path) -> Self {
        use std::sync::{Mutex, OnceLock};
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        let lock = LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .expect("chdir lock");
        let prior = std::env::current_dir().expect("cwd");
        std::env::set_current_dir(p).expect("chdir");
        Self { prior, _lock: lock }
    }
}
impl Drop for ChdirGuard {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.prior);
    }
}
