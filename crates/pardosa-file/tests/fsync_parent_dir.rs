//! Substrate-pure parent-directory fsync helper (G12 epic
//! `rescue-pardosa-kh4s`, task `rescue-pardosa-77m2`).
//!
//! Pins the public surface of `fsync_parent_dir`: returns `Ok(())`
//! when the directory exists and is openable on the host platform.
//! Non-existent directories surface as `io::Error` on Unix (the
//! recovery-relevant signal a caller would see if a `.pgno` parent
//! disappeared between path resolution and fsync). On Windows the
//! call is a no-op (POSIX directory-fsync has no FS-level analogue)
//! and the contract is documented as such.
use pardosa_file::fsync_parent_dir;
use std::fs;
use tempfile::tempdir;
#[test]
fn fsync_parent_dir_on_existing_directory_is_ok() {
    let dir = tempdir().expect("tempdir");
    fsync_parent_dir(dir.path()).expect("fsync_parent_dir on existing dir");
}
#[test]
fn fsync_parent_dir_on_dot_is_ok() {
    fsync_parent_dir(std::path::Path::new(".")).expect("fsync_parent_dir on .");
}
#[cfg(not(windows))]
#[test]
fn fsync_parent_dir_on_missing_directory_errors_on_unix() {
    let dir = tempdir().expect("tempdir");
    let ghost = dir.path().join("definitely-not-there");
    assert!(!ghost.exists(), "precondition");
    let err = fsync_parent_dir(&ghost).expect_err("missing dir must error on unix");
    assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
}
#[cfg(not(windows))]
#[test]
fn fsync_parent_dir_after_creating_a_file_inside_does_not_error() {
    let dir = tempdir().expect("tempdir");
    let f = dir.path().join("child.bin");
    fs::write(&f, b"x").expect("write child");
    fsync_parent_dir(dir.path()).expect("fsync_parent_dir after child create");
}
