use assert_cmd::Command;

const ORG: &str = "test-org-smoke";

#[test]
fn dump_baseline_empty_store_uses_pardosa_pgno_artifact() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let output = Command::cargo_bin("gh-report")
        .expect("locate gh-report binary")
        .args([
            "--dump-baseline",
            "--org",
            ORG,
            "--store-dir",
            tmp.path().to_str().expect("tempdir utf-8"),
            "--pardosa-backend",
            "pgno",
        ])
        .output()
        .expect("spawn gh-report");
    assert!(
        output.status.success(),
        "gh-report --dump-baseline exited {:?}; stderr=\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout utf-8");
    let baseline: serde_json::Value = serde_json::from_str(&stdout).expect("baseline JSON");
    assert!(baseline.get("schema_version").is_some());
    let events_dir = tmp.path().join("events").join(ORG);
    assert!(
        events_dir.join("events.pgno").is_file(),
        "pardosa .pgno artifact must exist under {}",
        events_dir.display()
    );
    assert!(
        !events_dir.join("1.msgpack").exists(),
        "M1 must not write the old MsgpackFileStore aggregate file"
    );
    assert!(
        !stdout.contains("tier-fail"),
        "empty dump-baseline smoke should not render a failing HTML tier in JSON output"
    );
}
