use std::fs;
use std::path::{Path, PathBuf};

use tempfile::tempdir;

fn write_workspace(root: &Path, alpha_description: &str) {
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/alpha\", \"crates/beta\"]\n",
    )
    .expect("write root Cargo.toml");

    fs::create_dir_all(root.join("crates/alpha")).expect("mkdir alpha");
    fs::write(
        root.join("crates/alpha/Cargo.toml"),
        format!(
            "[package]\nname = \"alpha\"\ndescription = \"{alpha_description}\"\n\n[dependencies]\nserde = \"1\"\n"
        ),
    )
    .expect("write alpha Cargo.toml");
    fs::write(root.join("crates/alpha/README.md"), "# alpha\n").expect("write alpha README");

    fs::create_dir_all(root.join("crates/beta")).expect("mkdir beta");
    fs::write(
        root.join("crates/beta/Cargo.toml"),
        "[package]\nname = \"beta\"\n",
    )
    .expect("write beta Cargo.toml");
}

fn extract_fenced<'a>(text: &'a str, section_id: &str) -> &'a str {
    let begin = format!("%% architect:begin {section_id} %%");
    let end = format!("%% architect:end {section_id} %%");
    let start = text.find(&begin).expect("begin marker present") + begin.len();
    let stop = text[start..].find(&end).expect("end marker present") + start;
    &text[start..stop]
}

#[test]
fn hand_edit_survives_regeneration_and_fenced_content_updates_idempotently() {
    let workspace = tempdir().expect("workspace tempdir");
    let out = tempdir().expect("out tempdir");
    write_workspace(workspace.path(), "Alpha crate v1");

    let config = architect::Config::new(
        workspace.path().to_path_buf(),
        out.path().to_path_buf(),
        Some(PathBuf::from("Code/test-repo")),
    );

    let report1 = architect::run(&config).expect("first run");
    assert_eq!(report1.written.len(), 4);

    let alpha_path = out.path().join("Code/test-repo/alpha.md");
    let content1 = fs::read_to_string(&alpha_path).expect("read alpha.md after run1");
    assert!(content1.contains("Alpha crate v1"));

    let header = "# My own notes on alpha\n\nKeep this.\n\n";
    let footer = "\n\nFooter I added by hand.\n";
    let hand_edited = format!("{header}{content1}{footer}");
    fs::write(&alpha_path, &hand_edited).expect("apply hand edit");

    let report2 = architect::run(&config).expect("second run, unchanged source");
    assert_eq!(report2.written.len(), 4);
    let content2 = fs::read_to_string(&alpha_path).expect("read alpha.md after run2");

    assert!(
        content2.starts_with(header),
        "hand-edited header must survive byte-for-byte: {content2}"
    );
    assert!(
        content2.ends_with(footer),
        "hand-edited footer must survive byte-for-byte: {content2}"
    );
    assert_eq!(
        extract_fenced(&content2, "alpha"),
        extract_fenced(&content1, "alpha"),
        "fenced content must be unchanged when source is unchanged"
    );

    write_workspace(workspace.path(), "Alpha crate v2");
    let report3 = architect::run(&config).expect("third run, source changed");
    assert_eq!(report3.written.len(), 4);
    let content3 = fs::read_to_string(&alpha_path).expect("read alpha.md after run3");

    assert!(
        content3.starts_with(header),
        "header must survive a run that updates the fence"
    );
    assert!(
        content3.ends_with(footer),
        "footer must survive a run that updates the fence"
    );
    assert!(extract_fenced(&content3, "alpha").contains("Alpha crate v2"));
    assert!(!extract_fenced(&content3, "alpha").contains("Alpha crate v1"));

    let report4 = architect::run(&config).expect("fourth run, source unchanged since run3");
    assert_eq!(report4.written.len(), 4);
    let content4 = fs::read_to_string(&alpha_path).expect("read alpha.md after run4");

    assert_eq!(content3, content4, "identical re-run must be byte-stable");
}

#[test]
fn missing_readme_and_missing_adrs_render_graceful_sections_without_failing_the_run() {
    let workspace = tempdir().expect("workspace tempdir");
    let out = tempdir().expect("out tempdir");
    write_workspace(workspace.path(), "Alpha crate v1");

    let config = architect::Config::new(
        workspace.path().to_path_buf(),
        out.path().to_path_buf(),
        None,
    );

    let report = architect::run(&config).expect("run tolerates gaps");
    let beta_path = report
        .written
        .iter()
        .find(|path| path.file_name().and_then(|name| name.to_str()) == Some("beta.md"))
        .expect("beta.md written");
    let beta_content = fs::read_to_string(beta_path).expect("read beta.md");

    assert!(beta_content.contains("No README."));
    assert!(beta_content.contains("No governing ADRs found."));
}
