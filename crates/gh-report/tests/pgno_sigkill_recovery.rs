#![forbid(unsafe_code)]

//! Synthetic SIGKILL crash-survival proof for `.pgno` footerless-tail recovery.

use std::fmt::Write as _;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Child, Command as ProcessCommand, Stdio};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;

use assert_cmd::Command;
use gh_report::app::state::EventStoreImpl;
use gh_report::event::DomainEvent;
use gh_report::infra::lock;
use pardosa_file::Reader;
use pardosa_schema::{NonEmptyEventString, Timestamp as EventTimestamp};

const CHILD_ENV: &str = "PGNO_SIGKILL_CHILD";
const STORE_DIR_ENV: &str = "PGNO_SIGKILL_STORE_DIR";
const MARKER_ENV: &str = "PGNO_SIGKILL_MARKER";
const CYCLE_ENV: &str = "PGNO_SIGKILL_CYCLE";
const EVENTS_PER_CYCLE_ENV: &str = "PGNO_SIGKILL_EVENTS_PER_CYCLE";
const ORG: &str = "sigkill-pgno-org";
const CYCLE_COUNT: usize = 5;
const EVENTS_PER_CYCLE: usize = 4;

#[derive(Debug)]
struct CycleRow {
    cycle: usize,
    events_written: usize,
    torn_tail: bool,
    rehydrate_ok: bool,
    loadfailed_count: usize,
    runlock_clean: bool,
    event_count: usize,
}

#[test]
fn sigkill_footerless_recovery_harness_runs() {
    if std::env::var_os(CHILD_ENV).is_some() {
        child_main();
    }
    parent_main();
}

fn parent_main() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut previous_event_count = 0;
    let mut rows = Vec::with_capacity(CYCLE_COUNT);

    for cycle in 1..=CYCLE_COUNT {
        let marker = tmp.path().join(format!("cycle-{cycle}.ready"));
        let mut child = spawn_child(tmp.path(), &marker, cycle);
        let events_written = wait_for_marker(&mut child, &marker);
        kill_child(&mut child);
        let torn_tail = assert_footerless_tail(&events_path(tmp.path()));
        let loadfailed_count = dump_baseline(tmp.path());
        let runlock_clean = !lock::lock_path(tmp.path()).exists();
        assert!(runlock_clean, "run-lock file leaked after cycle {cycle}");
        let event_count = recovered_event_count(tmp.path());
        assert!(
            event_count >= previous_event_count,
            "event count regressed on cycle {cycle}: previous={previous_event_count}, current={event_count}"
        );
        previous_event_count = event_count;
        rows.push(CycleRow {
            cycle,
            events_written,
            torn_tail,
            rehydrate_ok: true,
            loadfailed_count,
            runlock_clean,
            event_count,
        });
    }

    assert!(
        rows.iter().any(|row| row.torn_tail),
        "at least one cycle must prove footerless torn-tail recovery"
    );
    println!("{}", render_table(&rows));
}

fn child_main() -> ! {
    let store_dir = required_path_env(STORE_DIR_ENV);
    let marker = required_path_env(MARKER_ENV);
    let cycle = required_usize_env(CYCLE_ENV);
    let events_per_cycle = required_usize_env(EVENTS_PER_CYCLE_ENV);
    let events_dir = store_dir.join("events").join(ORG);
    std::fs::create_dir_all(&events_dir).expect("create events dir");
    let pgno = events_dir.join("events.pgno");

    if !pgno.exists() {
        let store = EventStoreImpl::create_pgno(&pgno).expect("create pgno bootstrap store");
        store
            .record("bootstrap", native_event("bootstrap", "sigkill-bootstrap"))
            .expect("record bootstrap event");
    }

    let store = EventStoreImpl::open_pgno(&pgno).expect("open pgno append store");
    let mut events_written = 0;
    for event_index in 0..events_per_cycle {
        let domain_key = format!("cycle-{cycle}-event-{event_index}");
        let repo_name = format!("sigkill-cycle-{cycle}-repo-{event_index}");
        store
            .record(&domain_key, native_event(&domain_key, &repo_name))
            .expect("record crash-window event");
        events_written += 1;
    }

    assert!(events_written >= 1, "child must land at least one sync");
    std::fs::write(marker, events_written.to_string()).expect("write ready marker");
    loop {
        thread::park();
    }
}

fn spawn_child(store_dir: &Path, marker: &Path, cycle: usize) -> Child {
    let current_exe = std::env::current_exe().expect("current test binary path");
    ProcessCommand::new(current_exe)
        .arg("--exact")
        .arg("sigkill_footerless_recovery_harness_runs")
        .arg("--nocapture")
        .env(CHILD_ENV, "1")
        .env(STORE_DIR_ENV, store_dir)
        .env(MARKER_ENV, marker)
        .env(CYCLE_ENV, cycle.to_string())
        .env(EVENTS_PER_CYCLE_ENV, EVENTS_PER_CYCLE.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn seed child process")
}

fn wait_for_marker(child: &mut Child, marker: &Path) -> usize {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(payload) = std::fs::read_to_string(marker) {
            return payload
                .trim()
                .parse()
                .expect("ready marker contains event count");
        }
        if let Some(status) = child.try_wait().expect("poll child") {
            panic!("seed child exited before ready marker: {status}");
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            panic!("seed child did not write ready marker before timeout");
        }
        thread::sleep(Duration::from_millis(25));
    }
}

fn kill_child(child: &mut Child) {
    child.kill().expect("send SIGKILL to seed child");
    let status = child.wait().expect("wait for seed child");
    #[cfg(unix)]
    assert_eq!(
        status.signal(),
        Some(9),
        "child must terminate via SIGKILL; got {status}"
    );
    #[cfg(not(unix))]
    assert!(
        !status.success(),
        "child must not exit cleanly after hard kill"
    );
}

fn assert_footerless_tail(pgno: &Path) -> bool {
    let manifest = manifest_path(pgno);
    assert!(
        manifest.exists(),
        "manifest sidecar missing at {}",
        manifest.display()
    );
    let file = File::open(pgno).expect("open pgno for footerless assertion");
    let reader_result = Reader::open(file);
    assert!(
        reader_result.is_err(),
        "Reader::open accepted pre-recovery footerless tail at {}",
        pgno.display()
    );
    true
}

fn dump_baseline(store_dir: &Path) -> usize {
    let output = Command::cargo_bin("gh-report")
        .expect("locate gh-report binary")
        .args([
            "--dump-baseline",
            "--org",
            ORG,
            "--store-dir",
            store_dir.to_str().expect("store dir is utf-8"),
        ])
        .output()
        .expect("spawn gh-report dump-baseline");
    let stderr = String::from_utf8(output.stderr).expect("stderr utf-8");
    let loadfailed_count = stderr.matches("LoadFailed").count();
    assert!(
        output.status.success(),
        "gh-report --dump-baseline failed with {:?}; stderr=\n{stderr}",
        output.status.code()
    );
    assert_eq!(
        loadfailed_count, 0,
        "dump-baseline stderr contained LoadFailed: {stderr}"
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout utf-8");
    let baseline: serde_json::Value =
        serde_json::from_str(&stdout).expect("stdout is parseable Baseline JSON");
    assert!(
        baseline.get("schema_version").is_some(),
        "Baseline missing schema_version; raw stdout=\n{stdout}"
    );
    assert!(
        baseline
            .get("entries")
            .and_then(serde_json::Value::as_object)
            .is_some(),
        "Baseline missing entries object; raw stdout=\n{stdout}"
    );
    loadfailed_count
}

fn recovered_event_count(store_dir: &Path) -> usize {
    EventStoreImpl::open_pgno(&events_path(store_dir))
        .expect("open recovered pgno")
        .events()
        .expect("read recovered events")
        .len()
}

fn events_path(store_dir: &Path) -> PathBuf {
    store_dir.join("events").join(ORG).join("events.pgno")
}

fn manifest_path(pgno: &Path) -> PathBuf {
    let mut os = pgno.as_os_str().to_os_string();
    os.push(".pgix");
    PathBuf::from(os)
}

fn required_path_env(key: &str) -> PathBuf {
    PathBuf::from(std::env::var_os(key).unwrap_or_else(|| panic!("missing {key}")))
}

fn required_usize_env(key: &str) -> usize {
    std::env::var(key)
        .unwrap_or_else(|_| panic!("missing {key}"))
        .parse()
        .unwrap_or_else(|_| panic!("{key} must be usize"))
}

fn render_table(rows: &[CycleRow]) -> String {
    let mut table = String::from(
        "cycle | events_written | kill_kind | torn_tail | rehydrate_ok | loadfailed_count | runlock_clean | event_count\n",
    );
    for row in rows {
        writeln!(
            &mut table,
            "{} | {} | SIGKILL | {} | {} | {} | {} | {}",
            row.cycle,
            row.events_written,
            yes_no(row.torn_tail),
            yes_no(row.rehydrate_ok),
            row.loadfailed_count,
            yes_no(row.runlock_clean),
            row.event_count
        )
        .expect("write table row");
    }
    table
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

fn native_event(domain_key: &str, repo_name: &str) -> DomainEvent {
    DomainEvent::RepositoryStateCaptured {
        domain_key: NonEmptyEventString::try_new(domain_key).expect("domain key"),
        repo_name: NonEmptyEventString::try_new(repo_name).expect("repo name"),
        timestamp: EventTimestamp::from_nanos(1_779_491_200_000_000_000).expect("timestamp"),
        evidence: None,
    }
}
