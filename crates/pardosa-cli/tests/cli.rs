use assert_cmd::Command;
use pardosa::store::{EventStore, PgnoBackend};
use pardosa_cli::DomainEvent;
use pardosa_cli::event::limits::{MAX_BATCH_ID, MAX_ORG};
use pardosa_schema::{NonEmptyEventString, Timestamp};
use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use tempfile::tempdir;
fn bin() -> Command {
    Command::cargo_bin("pardosa-cli").expect("binary built by cargo")
}

const PRESERVED_CORRUPT_PGNO: &str =
    "/var/folders/gl/p6twn53j4x12v_y21k0xc3780000gp/T/opencode/events.pgno.corrupt";
const PRESERVED_CORRUPT_PGIX: &str =
    "/var/folders/gl/p6twn53j4x12v_y21k0xc3780000gp/T/opencode/events.pgno.pgix.corrupt";

fn preserved_fixture_paths() -> (PathBuf, PathBuf) {
    let pgno = PathBuf::from(PRESERVED_CORRUPT_PGNO);
    let pgix = PathBuf::from(PRESERVED_CORRUPT_PGIX);
    assert!(
        pgno.exists(),
        "missing preserved .pgno fixture: {}",
        pgno.display()
    );
    assert!(
        pgix.exists(),
        "missing preserved .pgix fixture: {}",
        pgix.display()
    );
    (pgno, pgix)
}

fn copy_preserved_fixture(dir: &Path) -> PathBuf {
    let (pgno, pgix) = preserved_fixture_paths();
    let work = dir.join("events.pgno");
    fs::copy(pgno, &work).expect("copy preserved pgno");
    fs::copy(pgix, pgix_path(&work)).expect("copy preserved pgix");
    work
}

fn pgix_path(path: &Path) -> PathBuf {
    let mut os = path.as_os_str().to_os_string();
    os.push(".pgix");
    PathBuf::from(os)
}

fn nes<const MAX: usize>(s: &str) -> NonEmptyEventString<MAX> {
    NonEmptyEventString::try_new(s).expect("nonempty bounded string")
}

fn sample_event() -> DomainEvent {
    DomainEvent::SweepStarted {
        org: nes::<MAX_ORG>("o"),
        repo_count: 1,
        batch_id: nes::<MAX_BATCH_ID>("b"),
        timestamp: Timestamp::from_nanos(1).expect("nonzero timestamp"),
        snapshot_signature: None,
    }
}

fn create_footerless_manifest_store(path: &Path) {
    {
        let mut store = EventStore::<DomainEvent>::create(path).expect("create store");
        let _receipt = store.writer().begin(sample_event()).expect("begin");
        let _lsn = store.writer().sync().expect("sync direct seed");
    }
    {
        let mut store = EventStore::<DomainEvent>::open_with_backend(PgnoBackend::open(path))
            .expect("open backend store");
        let _receipt = store.writer().begin(sample_event()).expect("begin backend");
        let _lsn = store.writer().sync().expect("sync backend manifest");
    }
}

fn body_start(path: &Path) -> usize {
    let bytes = fs::read(path).expect("read pgno");
    let schema_size = u32::from_le_bytes(bytes[29..33].try_into().expect("schema size bytes"));
    40 + ((schema_size as usize + 7) & !7)
}

fn corrupt_first_body_byte(path: &Path) {
    let mut bytes = fs::read(path).expect("read pgno");
    let body = body_start(path);
    bytes[body] ^= 0xFF;
    fs::write(path, bytes).expect("write pgno");
}

fn rewrite_manifest_frontier(path: &Path, frontier: [u8; 32]) {
    let manifest_path = pgix_path(path);
    let mut manifest = fs::read(&manifest_path).expect("read manifest");
    let footer_start = manifest.len() - 60;
    manifest[footer_start + 16..footer_start + 48].copy_from_slice(&frontier);
    let mut hasher = xxhash_rust::xxh64::Xxh64::new(0);
    hasher.update(&manifest[..footer_start]);
    hasher.update(&frontier);
    manifest[footer_start + 48..footer_start + 56].copy_from_slice(&hasher.digest().to_le_bytes());
    fs::write(manifest_path, manifest).expect("write manifest");
}

fn count_gh_report_records(path: &Path) -> usize {
    let store: EventStore<gh_report::event::DomainEvent> =
        EventStore::<gh_report::event::DomainEvent>::open_with_backend(PgnoBackend::open(path))
            .expect("open recovered gh-report store");
    let sidecar = path.with_extension("cursor");
    let reader = store.reader();
    let mut cursor = reader.cursor(&sidecar).expect("cursor open");
    cursor
        .tail()
        .collect::<Result<Vec<_>, _>>()
        .expect("read recovered events")
        .len()
}

#[test]
fn recover_dry_run_reports_preserved_fixture_plan_without_mutating() {
    let dir = tempdir().unwrap();
    let store = copy_preserved_fixture(dir.path());
    let before = fs::read(&store).expect("read before dry-run");
    let plan = pardosa::store::plan_offline_pgno_recovery(&store).expect("plan fixture");
    let expected_truncated = plan.truncated_bytes;
    let out = bin()
        .arg("recover")
        .arg(&store)
        .arg("--dry-run")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(out).expect("stdout utf8");
    assert!(
        stdout.contains("records_preserved = 774"),
        "stdout: {stdout}"
    );
    assert!(
        stdout.contains(&format!("truncated_bytes = {expected_truncated}")),
        "stdout: {stdout}",
    );
    assert!(stdout.contains("would_write = false"), "stdout: {stdout}");
    let after = fs::read(&store).expect("read after dry-run");
    assert_eq!(after, before, "dry-run must leave .pgno bytes unchanged");
}

#[test]
fn recover_force_recovers_preserved_fixture_to_openable_774_record_store() {
    let dir = tempdir().unwrap();
    let store = copy_preserved_fixture(dir.path());
    let out = bin()
        .arg("recover")
        .arg(&store)
        .arg("--force")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(out).expect("stdout utf8");
    assert!(
        stdout.contains("recovered_records = 774"),
        "stdout: {stdout}"
    );
    assert_eq!(count_gh_report_records(&store), 774);
}

#[test]
fn recover_declines_durable_region_body_checksum_mismatch() {
    let dir = tempdir().unwrap();
    let store = dir.path().join("body-corrupt.pgno");
    create_footerless_manifest_store(&store);
    let before = fs::read(&store).expect("read before corrupt");
    corrupt_first_body_byte(&store);
    let corrupt = fs::read(&store).expect("read corrupt");
    let dry = bin().arg("recover").arg(&store).arg("--dry-run").assert();
    let dry_out = dry.get_output();
    assert!(
        !dry_out.status.success(),
        "dry-run should decline body corruption"
    );
    let dry_stderr = String::from_utf8_lossy(&dry_out.stderr);
    assert!(
        dry_stderr.contains("BodyChecksumMismatch"),
        "stderr: {dry_stderr}",
    );
    let force = bin().arg("recover").arg(&store).arg("--force").assert();
    let force_out = force.get_output();
    assert!(
        !force_out.status.success(),
        "force should decline body corruption"
    );
    let force_stderr = String::from_utf8_lossy(&force_out.stderr);
    assert!(
        force_stderr.contains("BodyChecksumMismatch"),
        "stderr: {force_stderr}",
    );
    assert_eq!(fs::read(&store).expect("read after force"), corrupt);
    assert_ne!(corrupt, before);
}

#[test]
fn recover_declines_frontier_mismatch() {
    let dir = tempdir().unwrap();
    let store = dir.path().join("frontier-mismatch.pgno");
    create_footerless_manifest_store(&store);
    let before = fs::read(&store).expect("read before frontier mismatch");
    let mut wrong = [0x5Au8; 32];
    wrong[0] ^= 0xA5;
    rewrite_manifest_frontier(&store, wrong);
    let result = bin().arg("recover").arg(&store).arg("--force").assert();
    let out = result.get_output();
    assert!(
        !out.status.success(),
        "force should decline frontier mismatch"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("FrontierMismatch"), "stderr: {stderr}");
    assert_eq!(fs::read(&store).expect("read after decline"), before);
}

#[test]
fn recover_without_force_or_dry_run_is_gated() {
    let dir = tempdir().unwrap();
    let store = dir.path().join("gated.pgno");
    create_footerless_manifest_store(&store);
    let result = bin().arg("recover").arg(&store).assert();
    let out = result.get_output();
    assert!(
        !out.status.success(),
        "recover must require --dry-run or --force"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--force"), "stderr: {stderr}");
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
