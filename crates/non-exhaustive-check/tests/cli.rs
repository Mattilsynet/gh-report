#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};

struct Fixture(PathBuf);

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.0) {
            eprintln!("fixture cleanup failed for {}: {error}", self.0.display());
        }
    }
}

fn check(root: &Path, code: i32, diagnostic: &str) {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_non-exhaustive-check"))
        .arg(root)
        .output()
        .expect("checker must execute");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.status.code(), Some(code), "{text}");
    assert!(text.contains(diagnostic), "{text}");
}

#[test]
fn cli_plant_fail_revert_clean() {
    let root = std::env::temp_dir().join(format!(
        "gh-report-checker-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();
    let _fixture = Fixture(root.clone());
    let file = root.join("lib.rs");
    std::fs::write(&file, "pub enum Clean { A }").unwrap();
    check(&root, 0, "0 violations");
    for (attribute, code) in [
        ("", 1),
        ("#[cfg(test)]", 0),
        ("#[cfg(not(test))]", 1),
        ("#[cfg(all(test, feature = \"x\"))]", 0),
        ("#[cfg(any(test, feature = \"x\"))]", 1),
        ("#[cfg(test, broken())]", 1),
    ] {
        std::fs::write(&file, format!("{attribute} #[derive(thiserror::Error)] #[non_exhaustive] pub enum PlantedError {{ A }}")).unwrap();
        check(
            &root,
            code,
            if code == 0 {
                "0 violations"
            } else {
                "PlantedError"
            },
        );
        std::fs::write(&file, "pub enum Clean { A }").unwrap();
        check(&root, 0, "0 violations");
    }
    std::fs::write(&file, "pub enum Broken {").unwrap();
    check(&root, 101, "failed to parse");
    std::fs::write(&file, "pub enum Clean { A }").unwrap();
    check(&root, 0, "0 violations");
    std::fs::remove_file(&file).unwrap();
    check(&root, 101, "no Rust sources");
    std::fs::remove_dir(&root).unwrap();
    check(&root, 101, "missing source directory");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(&file, "pub enum Clean { A }").unwrap();
    check(&root, 0, "0 violations");
}
