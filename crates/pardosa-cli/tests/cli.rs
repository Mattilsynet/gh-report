use assert_cmd::Command;
use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use tempfile::tempdir;
fn bin() -> Command {
    Command::cargo_bin("pardosa-cli").expect("binary built by cargo")
}
#[test]
fn sweep_start_writes_non_empty_file() {
    let dir = tempdir().unwrap();
    let store = dir.path().join("a.pgno");
    bin()
        .args([
            "sweep-start",
            "--org",
            "o",
            "--batch",
            "b1",
            "--repos",
            "3",
            "--store",
        ])
        .arg(&store)
        .assert()
        .success();
    let len = fs::metadata(&store).unwrap().len();
    assert!(len > 0, "store file should be non-empty, was {len} bytes");
}
#[test]
fn three_event_sequence_then_read_inspect_verify() {
    let dir = tempdir().unwrap();
    let store = dir.path().join("seq.pgno");
    bin()
        .args([
            "sweep-start",
            "--org",
            "o",
            "--batch",
            "b1",
            "--repos",
            "3",
            "--store",
        ])
        .arg(&store)
        .assert()
        .success();
    bin()
        .args([
            "repo-evaluated",
            "--repo",
            "r1",
            "--outcome",
            "success",
            "--duration-ms",
            "100",
            "--store",
        ])
        .arg(&store)
        .assert()
        .success();
    bin()
        .args([
            "sweep-complete",
            "--batch",
            "b1",
            "--duration-ms",
            "2000",
            "--store",
        ])
        .arg(&store)
        .assert()
        .success();
    let read_out = bin()
        .arg("read")
        .arg(&store)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(read_out).unwrap();
    assert!(stdout.contains("SweepStarted"), "read stdout: {stdout}");
    assert!(stdout.contains("RepoEvaluated"), "read stdout: {stdout}");
    assert!(stdout.contains("SweepCompleted"), "read stdout: {stdout}");
    let inspect_out = bin()
        .arg("inspect")
        .arg(&store)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let inspect = String::from_utf8(inspect_out).unwrap();
    assert!(
        inspect.contains("message_count = 3"),
        "inspect stdout: {inspect}"
    );
    assert!(inspect.contains("schema_hash"), "inspect stdout: {inspect}");
}
#[test]
fn repo_evaluated_alone_writes_file() {
    let dir = tempdir().unwrap();
    let store = dir.path().join("re.pgno");
    bin()
        .args([
            "repo-evaluated",
            "--repo",
            "r1",
            "--outcome",
            "success",
            "--duration-ms",
            "100",
            "--store",
        ])
        .arg(&store)
        .assert()
        .success();
    assert!(fs::metadata(&store).unwrap().len() > 0);
}
#[test]
fn repo_evaluated_outcome_failure_round_trips_success_false() {
    let dir = tempdir().unwrap();
    let store = dir.path().join("fail.pgno");
    bin()
        .args([
            "repo-evaluated",
            "--repo",
            "r1",
            "--outcome",
            "failure",
            "--duration-ms",
            "100",
            "--store",
        ])
        .arg(&store)
        .assert()
        .success();
    let read_out = bin()
        .arg("read")
        .arg(&store)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(read_out).unwrap();
    assert!(stdout.contains("RepoEvaluated"), "read stdout: {stdout}");
    assert!(
        stdout.contains("success: false"),
        "failure outcome must round-trip as success: false; got: {stdout}"
    );
}
#[test]
fn repo_evaluated_outcome_success_round_trips_success_true() {
    let dir = tempdir().unwrap();
    let store = dir.path().join("ok.pgno");
    bin()
        .args([
            "repo-evaluated",
            "--repo",
            "r1",
            "--outcome",
            "success",
            "--duration-ms",
            "100",
            "--store",
        ])
        .arg(&store)
        .assert()
        .success();
    let read_out = bin()
        .arg("read")
        .arg(&store)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(read_out).unwrap();
    assert!(stdout.contains("RepoEvaluated"), "read stdout: {stdout}");
    assert!(
        stdout.contains("success: true"),
        "success outcome must round-trip as success: true; got: {stdout}"
    );
}
#[test]
fn repo_evaluated_missing_outcome_rejected() {
    let dir = tempdir().unwrap();
    let store = dir.path().join("missing.pgno");
    let result = bin()
        .args([
            "repo-evaluated",
            "--repo",
            "r1",
            "--duration-ms",
            "100",
            "--store",
        ])
        .arg(&store)
        .assert();
    let out = result.get_output();
    assert!(
        !out.status.success(),
        "missing --outcome must fail; stdout={:?} stderr={:?}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    assert!(
        !store.exists(),
        "no .pgno should be created when arg parsing fails"
    );
}
#[test]
fn sweep_complete_alone_writes_file() {
    let dir = tempdir().unwrap();
    let store = dir.path().join("sc.pgno");
    bin()
        .args([
            "sweep-complete",
            "--batch",
            "b1",
            "--duration-ms",
            "2000",
            "--store",
        ])
        .arg(&store)
        .assert()
        .success();
    assert!(fs::metadata(&store).unwrap().len() > 0);
}
#[test]
fn tamper_byte_flip_fails_read() {
    let dir = tempdir().unwrap();
    let store = dir.path().join("tamper.pgno");
    bin()
        .args([
            "sweep-start",
            "--org",
            "o",
            "--batch",
            "b1",
            "--repos",
            "3",
            "--store",
        ])
        .arg(&store)
        .assert()
        .success();
    bin()
        .args([
            "sweep-complete",
            "--batch",
            "b1",
            "--duration-ms",
            "2000",
            "--store",
        ])
        .arg(&store)
        .assert()
        .success();
    bin().arg("read").arg(&store).assert().success();
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&store)
        .unwrap();
    let file_len = file.metadata().unwrap().len();
    assert!(file_len > 100, "file too small to tamper: {file_len}");
    let tamper_offset: u64 = 80;
    file.seek(SeekFrom::Start(tamper_offset)).unwrap();
    let mut byte = [0u8; 1];
    file.read_exact(&mut byte).unwrap();
    byte[0] ^= 0xFF;
    file.seek(SeekFrom::Start(tamper_offset)).unwrap();
    file.write_all(&byte).unwrap();
    drop(file);
    let result = bin().arg("read").arg(&store).assert();
    let out = result.get_output();
    assert!(
        !out.status.success(),
        "read should fail after tamper; stdout={:?} stderr={:?}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}
