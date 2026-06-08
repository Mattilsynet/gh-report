use std::fs;
use std::path::Path;
use std::process::Command;
const fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_comment-free")
}
fn run(root: &Path) -> std::process::Output {
    Command::new(bin())
        .arg("--rewrite")
        .arg("--force-dirty")
        .arg(root)
        .output()
        .expect("failed to spawn comment-free")
}
fn run_dry(root: &Path) -> std::process::Output {
    Command::new(bin())
        .arg("--rewrite")
        .arg("--dry-run")
        .arg(root)
        .output()
        .expect("failed to spawn comment-free")
}
fn write(dir: &Path, name: &str, content: &str) {
    let src = dir.join("src");
    fs::create_dir_all(&src).expect("mkdir src");
    fs::write(src.join(name), content).expect("write fixture");
}
fn read(dir: &Path, name: &str) -> String {
    fs::read_to_string(dir.join("src").join(name)).expect("read fixture")
}
#[test]
fn preserves_auto_trait_policy_markers() {
    let td = tempfile::tempdir().unwrap();
    let original = "// AUTO-TRAIT-POLICY-BEGIN\n\
                    // Mission rescue-pardosa-59y0: bucket every pub type.\n\
                    assert_auto_traits! {\n    \
                        SendSync { Foo, Bar }\n    \
                        SendOnly { }\n    \
                        NotSend { }\n\
                    }\n\
                    // AUTO-TRAIT-POLICY-END\n\
                    pub struct Foo;\n\
                    pub struct Bar;\n";
    write(td.path(), "lib.rs", original);
    run(td.path());
    let out = read(td.path(), "lib.rs");
    assert!(
        out.contains("AUTO-TRAIT-POLICY-BEGIN"),
        "BEGIN marker missing after rewrite:\n{out}"
    );
    assert!(
        out.contains("AUTO-TRAIT-POLICY-END"),
        "END marker missing after rewrite:\n{out}"
    );
    let begin = out.find("AUTO-TRAIT-POLICY-BEGIN").unwrap();
    let end = out.find("AUTO-TRAIT-POLICY-END").unwrap();
    assert!(begin < end, "markers in wrong order:\n{out}");
    let between = &out[begin..end];
    assert!(
        between.contains("assert_auto_traits"),
        "assert_auto_traits! not between markers:\n{out}"
    );
    assert!(
        !out.contains("Mission rescue-pardosa-59y0"),
        "ordinary line comment leaked through marker preservation:\n{out}"
    );
}
#[test]
fn preserves_auto_trait_policy_markers_around_multiple_macro_blocks() {
    let td = tempfile::tempdir().unwrap();
    let original = "// AUTO-TRAIT-POLICY-BEGIN\n\
                    // Mission rescue-pardosa-59y0: bucket every pub type.\n\
                    assert_auto_traits! {\n    \
                        SendSync { Foo, Bar }\n    \
                        SendOnly { }\n    \
                        NotSend { }\n\
                    }\n\
                    #[cfg(any(test, feature = \"test-support\"))]\n\
                    assert_auto_traits! {\n    \
                        SendSync { Gated }\n\
                    }\n\
                    // AUTO-TRAIT-POLICY-END\n\
                    pub struct Foo;\n\
                    pub struct Bar;\n\
                    #[cfg(any(test, feature = \"test-support\"))]\n\
                    pub struct Gated;\n";
    write(td.path(), "lib.rs", original);
    run(td.path());
    let out = read(td.path(), "lib.rs");
    assert!(
        out.contains("AUTO-TRAIT-POLICY-BEGIN"),
        "BEGIN marker missing after rewrite:\n{out}"
    );
    assert!(
        out.contains("AUTO-TRAIT-POLICY-END"),
        "END marker missing after rewrite:\n{out}"
    );
    let begin = out.find("AUTO-TRAIT-POLICY-BEGIN").unwrap();
    let end = out.find("AUTO-TRAIT-POLICY-END").unwrap();
    assert!(begin < end, "markers in wrong order:\n{out}");
    let between = &out[begin..end];
    let macro_count = between.matches("assert_auto_traits").count();
    assert_eq!(
        macro_count, 2,
        "expected both assert_auto_traits! blocks between markers, found {macro_count}:\n{out}"
    );
    assert!(
        between.contains("cfg(any(test, feature = \"test-support\"))")
            || between.contains("cfg (any (test , feature = \"test-support\"))")
            || between.contains("test-support"),
        "cfg-gated second block must stay inside markers:\n{out}"
    );
    assert!(
        !out.contains("Mission rescue-pardosa-59y0"),
        "ordinary line comment leaked through marker preservation:\n{out}"
    );
}
#[test]
fn auto_trait_policy_markers_unaffected_when_absent() {
    let td = tempfile::tempdir().unwrap();
    write(td.path(), "lib.rs", "// kill me\nfn f() {}\n");
    run(td.path());
    let out = read(td.path(), "lib.rs");
    assert!(
        !out.contains("AUTO-TRAIT-POLICY"),
        "marker spuriously injected:\n{out}"
    );
    assert!(!out.contains("kill me"), "// not stripped:\n{out}");
}
#[test]
fn preserves_outer_line_doc() {
    let td = tempfile::tempdir().unwrap();
    write(td.path(), "a.rs", "/// outer doc\nfn f() {}\n");
    run(td.path());
    let out = read(td.path(), "a.rs");
    assert!(out.contains("outer doc"), "outer /// lost:\n{out}");
}
#[test]
fn preserves_inner_line_doc() {
    let td = tempfile::tempdir().unwrap();
    write(td.path(), "a.rs", "//! crate-level inner doc\nfn f() {}\n");
    run(td.path());
    let out = read(td.path(), "a.rs");
    assert!(out.contains("crate-level inner doc"), "//! lost:\n{out}");
}
#[test]
fn preserves_explicit_doc_attr() {
    let td = tempfile::tempdir().unwrap();
    write(
        td.path(),
        "a.rs",
        "#[doc = \"explicit doc payload\"]\nfn f() {}\n",
    );
    run(td.path());
    let out = read(td.path(), "a.rs");
    assert!(
        out.contains("explicit doc payload"),
        "#[doc=\"...\"] lost:\n{out}"
    );
}
#[test]
fn preserves_doc_hidden() {
    let td = tempfile::tempdir().unwrap();
    write(td.path(), "a.rs", "#[doc(hidden)]\npub fn f() {}\n");
    run(td.path());
    let out = read(td.path(), "a.rs");
    assert!(
        out.contains("doc(hidden)") || out.contains("doc (hidden)"),
        "#[doc(hidden)] lost:\n{out}"
    );
}
#[test]
fn preserves_cfg_attr_doc() {
    let td = tempfile::tempdir().unwrap();
    write(
        td.path(),
        "a.rs",
        "#[cfg_attr(test, doc = \"gated doc payload\")]\nfn f() {}\n",
    );
    run(td.path());
    let out = read(td.path(), "a.rs");
    assert!(
        out.contains("gated doc payload"),
        "cfg_attr doc payload lost:\n{out}"
    );
}
#[test]
fn preserves_doc_inside_macro_rules() {
    let td = tempfile::tempdir().unwrap();
    write(
        td.path(),
        "a.rs",
        "macro_rules! m {\n    () => {\n        /// inside macro doc\n        fn g() {}\n    };\n}\n",
    );
    run(td.path());
    let out = read(td.path(), "a.rs");
    assert!(
        out.contains("inside macro doc") || out.contains("# [doc"),
        "doc inside macro_rules lost:\n{out}"
    );
}
#[test]
fn preserves_outer_doc_on_field_and_variant() {
    let td = tempfile::tempdir().unwrap();
    write(
        td.path(),
        "a.rs",
        "struct S {\n    /// field doc\n    x: u8,\n}\n\nenum E {\n    /// variant doc\n    V,\n}\n",
    );
    run(td.path());
    let out = read(td.path(), "a.rs");
    assert!(out.contains("field doc"), "field doc lost:\n{out}");
    assert!(out.contains("variant doc"), "variant doc lost:\n{out}");
}
#[test]
fn strips_line_comment_above_item() {
    let td = tempfile::tempdir().unwrap();
    write(td.path(), "a.rs", "// kill me line comment\nfn f() {}\n");
    run(td.path());
    let out = read(td.path(), "a.rs");
    assert!(!out.contains("kill me"), "// survived:\n{out}");
}
#[test]
fn strips_block_comment() {
    let td = tempfile::tempdir().unwrap();
    write(
        td.path(),
        "a.rs",
        "/* kill me block */\nfn f() { let _x = /* inline kill */ 1; }\n",
    );
    run(td.path());
    let out = read(td.path(), "a.rs");
    assert!(!out.contains("kill me"), "/* */ survived:\n{out}");
    assert!(
        !out.contains("inline kill"),
        "inline /* */ survived:\n{out}"
    );
}
#[test]
fn strips_line_comment_inside_fn_body() {
    let td = tempfile::tempdir().unwrap();
    write(
        td.path(),
        "a.rs",
        "fn f() {\n    // kill me inner\n    let _x = 1;\n}\n",
    );
    run(td.path());
    let out = read(td.path(), "a.rs");
    assert!(!out.contains("kill me inner"), "inner // survived:\n{out}");
}
#[test]
fn strips_line_comment_inside_macro_invocation() {
    let td = tempfile::tempdir().unwrap();
    write(
        td.path(),
        "a.rs",
        "fn f() {\n    println!(\n        \"x\" // kill me macro arg\n    );\n}\n",
    );
    run(td.path());
    let out = read(td.path(), "a.rs");
    assert!(
        !out.contains("kill me macro arg"),
        "// in macro survived:\n{out}"
    );
}
#[test]
fn strips_non_doc_but_preserves_doc_in_same_file() {
    let td = tempfile::tempdir().unwrap();
    write(
        td.path(),
        "a.rs",
        "//! keep inner\n\n// kill outer line\n/// keep outer\nfn f() {\n    // kill inner line\n    /* kill block */\n    let _x = 1;\n}\n",
    );
    run(td.path());
    let out = read(td.path(), "a.rs");
    assert!(out.contains("keep inner"), "//! lost:\n{out}");
    assert!(out.contains("keep outer"), "/// lost:\n{out}");
    assert!(
        !out.contains("kill outer line"),
        "outer // survived:\n{out}"
    );
    assert!(
        !out.contains("kill inner line"),
        "inner // survived:\n{out}"
    );
    assert!(!out.contains("kill block"), "/* */ survived:\n{out}");
}
#[test]
fn leaves_unparseable_file_untouched() {
    let td = tempfile::tempdir().unwrap();
    let original = "/// keep doc on broken file\n// kill on broken file\nfn f() {\n";
    write(td.path(), "broken.rs", original);
    let out = run(td.path());
    let after = read(td.path(), "broken.rs");
    assert_eq!(after, original, "unparseable file was modified");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("PARSE_ERROR"),
        "expected PARSE_ERROR diagnostic, got: {stderr}"
    );
}
#[test]
fn dry_run_does_not_modify_files() {
    let td = tempfile::tempdir().unwrap();
    let original = "// kill me\nfn f() {}\n";
    write(td.path(), "a.rs", original);
    let _ = run_dry(td.path());
    let after = read(td.path(), "a.rs");
    assert_eq!(after, original, "dry-run modified the file on disk");
}
#[test]
fn dry_run_emits_unified_diff() {
    let td = tempfile::tempdir().unwrap();
    let original = "// kill me\nfn f() {}\n";
    write(td.path(), "a.rs", original);
    let out = run_dry(td.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("WOULD_REWRITE"),
        "no WOULD_REWRITE tag:\n{stdout}"
    );
    assert!(
        stdout.contains("--- a/"),
        "no unified-diff '---' header:\n{stdout}"
    );
    assert!(
        stdout.contains("+++ b/"),
        "no unified-diff '+++' header:\n{stdout}"
    );
    assert!(stdout.contains("@@"), "no hunk marker:\n{stdout}");
    assert!(
        stdout.contains("-// kill me"),
        "removed line not shown in diff:\n{stdout}"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("mode=dry-run"),
        "summary missing mode=dry-run on stderr:\n{stderr}"
    );
}
#[test]
fn dry_run_short_flag_works() {
    let td = tempfile::tempdir().unwrap();
    let original = "// kill me\nfn f() {}\n";
    write(td.path(), "a.rs", original);
    let out = Command::new(bin())
        .arg("--rewrite")
        .arg("-n")
        .arg(td.path())
        .output()
        .expect("spawn");
    let after = read(td.path(), "a.rs");
    assert_eq!(after, original, "-n modified the file");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("WOULD_REWRITE"),
        "-n did not produce WOULD_REWRITE:\n{stdout}"
    );
}
#[test]
fn dry_run_unchanged_file_emits_no_diff() {
    let td = tempfile::tempdir().unwrap();
    write(td.path(), "a.rs", "");
    let out = run_dry(td.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("WOULD_REWRITE"),
        "spurious WOULD_REWRITE for empty file:\n{stdout}"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("unchanged=1"),
        "summary did not count file as unchanged on stderr:\n{stderr}"
    );
}
#[test]
fn write_mode_summary_says_mode_write() {
    let td = tempfile::tempdir().unwrap();
    write(td.path(), "a.rs", "// kill me\nfn f() {}\n");
    let out = run(td.path());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("mode=write"),
        "summary missing mode=write on stderr:\n{stderr}"
    );
}
#[test]
fn doc_warn_emits_when_root_is_dot() {
    let td = tempfile::tempdir().unwrap();
    fs::write(td.path().join("README.md"), "hi\n").expect("write README");
    write(td.path(), "a.rs", "fn f() {}\n");
    let out = Command::new(bin())
        .arg("--rewrite")
        .arg("--dry-run")
        .arg(".")
        .current_dir(td.path())
        .output()
        .expect("failed to spawn comment-free");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("DOC_WARN") && stderr.contains("README.md"),
        "DOC_WARN missing when ROOT='.':\n{stderr}"
    );
}
#[test]
fn scan_doc_files_skips_vendor_dirs() {
    let td = tempfile::tempdir().unwrap();
    fs::write(td.path().join("README.md"), "root\n").expect("write README");
    for sub in ["node_modules", "vendor", "dist", "build", "target"] {
        std::fs::create_dir_all(td.path().join(sub)).expect("mkdir");
        std::fs::write(td.path().join(sub).join("README.md"), "nested\n").expect("write");
    }
    write(td.path(), "a.rs", "fn f() {}\n");
    let out = Command::new(bin())
        .arg("--rewrite")
        .arg("--dry-run")
        .arg(".")
        .current_dir(td.path())
        .output()
        .expect("failed to spawn comment-free");
    let stderr = String::from_utf8_lossy(&out.stderr);
    let warn_count = stderr.matches("DOC_WARN").count();
    assert_eq!(
        warn_count, 1,
        "expected exactly 1 DOC_WARN (root README.md), got {warn_count}:\n{stderr}"
    );
    for sub in ["node_modules", "vendor", "dist", "build", "target"] {
        assert!(
            !stderr.contains(sub),
            "DOC_WARN unexpectedly reported file under {sub}/:\n{stderr}"
        );
    }
}
/// Initialise a git repo at `dir`, configure user, and make an initial commit.
fn git_init_repo(dir: &Path) {
    let run = |args: &[&str]| {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .expect("git");
        assert!(out.status.success(), "git {args:?} failed: {out:?}");
    };
    run(&["init", "-q", "-b", "main"]);
    run(&["config", "user.email", "test@example.invalid"]);
    run(&["config", "user.name", "test"]);
    run(&["config", "commit.gpgsign", "false"]);
    fs::write(dir.join(".gitkeep"), "").expect("write");
    run(&["add", ".gitkeep"]);
    run(&["commit", "-q", "-m", "init"]);
}
#[test]
fn git_state_ignores_dirty_sibling() {
    let td = tempfile::tempdir().unwrap();
    git_init_repo(td.path());
    fs::create_dir_all(td.path().join("src")).expect("mkdir");
    fs::create_dir_all(td.path().join("sibling")).expect("mkdir");
    fs::write(td.path().join("src/a.rs"), "fn f() {}\n").expect("write");
    let _ = Command::new("git")
        .args(["add", "."])
        .current_dir(td.path())
        .output();
    let _ = Command::new("git")
        .args(["commit", "-q", "-m", "seed"])
        .current_dir(td.path())
        .output();
    fs::write(td.path().join("sibling/dirty.rs"), "fn x() {}\n").expect("write");
    let out = Command::new(bin())
        .arg("--rewrite")
        .arg(td.path().join("src"))
        .output()
        .expect("failed to spawn comment-free");
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "expected exit 0 (clean scoped subtree), got {:?}\nstderr: {stderr}\nstdout: {stdout}",
        out.status.code()
    );
}
#[test]
fn dry_run_without_rewrite_is_rejected() {
    let td = tempfile::tempdir().unwrap();
    write(td.path(), "a.rs", "fn f() {}\n");
    let out = Command::new(bin())
        .arg("--dry-run")
        .arg(td.path())
        .output()
        .expect("failed to spawn comment-free");
    assert_eq!(
        out.status.code(),
        Some(2),
        "clap should require --rewrite alongside --dry-run (exit 2), got {:?}",
        out.status.code()
    );
}
#[test]
fn force_dirty_without_rewrite_is_rejected() {
    let td = tempfile::tempdir().unwrap();
    write(td.path(), "a.rs", "fn f() {}\n");
    let out = Command::new(bin())
        .arg("--force-dirty")
        .arg(td.path())
        .output()
        .expect("failed to spawn comment-free");
    assert_eq!(
        out.status.code(),
        Some(2),
        "clap should require --rewrite alongside --force-dirty (exit 2), got {:?}",
        out.status.code()
    );
}
#[test]
fn strip_with_parse_error_exits_five() {
    let td = tempfile::tempdir().unwrap();
    write(td.path(), "broken.rs", "fn f( {\n");
    write(td.path(), "ok.rs", "fn g() {}\n");
    let out = run(td.path());
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(5),
        "strip-mode per-file error must exit 5:\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        stderr.contains("PARSE_ERROR"),
        "missing PARSE_ERROR diagnostic:\n{stderr}"
    );
}
fn run_lint(root: &Path) -> std::process::Output {
    Command::new(bin())
        .arg(root)
        .output()
        .expect("failed to spawn comment-free")
}
fn run_lint_budget(root: &Path, budget: usize) -> std::process::Output {
    Command::new(bin())
        .arg(format!("--doc-max-words={budget}"))
        .arg(root)
        .output()
        .expect("failed to spawn comment-free")
}
#[test]
fn default_mode_is_lint() {
    let td = tempfile::tempdir().unwrap();
    let doc = "/// w01 w02 w03 w04 w05 w06 w07 w08 w09 w10\n\
               /// w11 w12 w13 w14 w15 w16 w17 w18 w19 w20\n\
               /// w21 w22 w23 w24 w25 w26 w27 w28 w29 w30\n\
               /// w31 w32 w33 w34 w35 w36 w37 w38 w39 w40\n\
               /// w41 w42 w43 w44 w45 w46 w47 w48 w49 w50\n\
               /// w51 w52 w53 w54 w55 w56 w57 w58 w59 w60\n\
               /// w61 w62 w63 w64 w65 w66 w67 w68 w69 w70\n\
               /// w71 w72 w73 w74 w75 w76 w77 w78 w79 w80\n\
               /// w81 w82 w83 w84 w85 w86 w87 w88 w89 w90\n\
               pub fn f() {}\n";
    write(td.path(), "a.rs", doc);
    let out = Command::new(bin())
        .arg(td.path())
        .output()
        .expect("failed to spawn comment-free");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(4),
        "default mode must be lint, expected exit 4, got {:?}\nstdout: {stdout}\nstderr: {stderr}",
        out.status.code()
    );
    assert!(stdout.contains("DOC_LINT\t"), "missing DOC_LINT:\n{stdout}");
    let after = read(td.path(), "a.rs");
    assert_eq!(after, doc, "default mode must not modify files");
}
#[test]
fn lint_within_budget_exits_zero() {
    let td = tempfile::tempdir().unwrap();
    write(
        td.path(),
        "a.rs",
        "/// one two three four five six seven eight nine ten\npub fn f() {}\n",
    );
    let out = run_lint(td.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(0),
        "expected exit 0, got {:?}\nstdout: {stdout}\nstderr: {stderr}",
        out.status.code()
    );
    assert!(
        !stdout.contains("DOC_LINT"),
        "no DOC_LINT expected within budget:\n{stdout}"
    );
}
#[test]
fn lint_over_budget_exits_four() {
    let td = tempfile::tempdir().unwrap();
    let doc = "/// w01 w02 w03 w04 w05 w06 w07 w08 w09 w10\n\
               /// w11 w12 w13 w14 w15 w16 w17 w18 w19 w20\n\
               /// w21 w22 w23 w24 w25 w26 w27 w28 w29 w30\n\
               /// w31 w32 w33 w34 w35 w36 w37 w38 w39 w40\n\
               /// w41 w42 w43 w44 w45 w46 w47 w48 w49 w50\n\
               /// w51 w52 w53 w54 w55 w56 w57 w58 w59 w60\n\
               /// w61 w62 w63 w64 w65 w66 w67 w68 w69 w70\n\
               /// w71 w72 w73 w74 w75 w76 w77 w78 w79 w80\n\
               /// w81 w82 w83 w84 w85 w86 w87 w88 w89 w90\n\
               pub fn f() {}\n";
    write(td.path(), "a.rs", doc);
    let out = run_lint(td.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(4),
        "expected exit 4, got {:?}\nstdout: {stdout}\nstderr: {stderr}",
        out.status.code()
    );
    assert!(stdout.contains("DOC_LINT\t"), "missing record:\n{stdout}");
    assert!(stdout.contains("words=90"), "wrong words field:\n{stdout}");
    assert!(
        stdout.contains("budget=80"),
        "wrong budget field:\n{stdout}"
    );
}
#[test]
fn lint_over_budget_emits_doctrine_message() {
    let td = tempfile::tempdir().unwrap();
    let doc = "/// w01 w02 w03 w04 w05 w06 w07 w08 w09 w10\n\
               /// w11 w12 w13 w14 w15 w16 w17 w18 w19 w20\n\
               /// w21 w22 w23 w24 w25 w26 w27 w28 w29 w30\n\
               /// w31 w32 w33 w34 w35 w36 w37 w38 w39 w40\n\
               /// w41 w42 w43 w44 w45 w46 w47 w48 w49 w50\n\
               /// w51 w52 w53 w54 w55 w56 w57 w58 w59 w60\n\
               /// w61 w62 w63 w64 w65 w66 w67 w68 w69 w70\n\
               /// w71 w72 w73 w74 w75 w76 w77 w78 w79 w80\n\
               /// w81 w82 w83 w84 w85 w86 w87 w88 w89 w90\n\
               pub fn f() {}\n";
    write(td.path(), "a.rs", doc);
    let out = run_lint(td.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("DOC_LINT_MSG\t"),
        "expected DOC_LINT_MSG tag after finding:\n{stdout}"
    );
    assert!(
        stdout.contains("Rust docs must contain a concise summary"),
        "doctrine sentence prefix missing:\n{stdout}"
    );
    assert!(
        stdout.contains("0-3 clear code examples"),
        "doctrine must mention 0-3 fenced examples:\n{stdout}"
    );
    assert!(
        stdout.contains("Fenced code examples are excluded from the prose word length"),
        "doctrine must state fenced examples are excluded from word length:\n{stdout}"
    );
    assert!(
        stdout.contains("references to ADRs must be given"),
        "doctrine sentence suffix missing:\n{stdout}"
    );
}
#[test]
fn lint_fenced_code_excluded() {
    let td = tempfile::tempdir().unwrap();
    let doc = "/// p01 p02 p03 p04 p05 p06 p07 p08 p09 p10\n\
               /// ```\n\
               /// c01 c02 c03 c04 c05 c06 c07 c08 c09 c10\n\
               /// c11 c12 c13 c14 c15 c16 c17 c18 c19 c20\n\
               /// c21 c22 c23 c24 c25 c26 c27 c28 c29 c30\n\
               /// c31 c32 c33 c34 c35 c36 c37 c38 c39 c40\n\
               /// c41 c42 c43 c44 c45 c46 c47 c48 c49 c50\n\
               /// ```\n\
               pub fn f() {}\n";
    write(td.path(), "a.rs", doc);
    let out = run_lint(td.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "stdout:\n{stdout}");
    assert!(
        !stdout.contains("DOC_LINT"),
        "fenced code should be excluded:\n{stdout}"
    );
}
#[test]
fn lint_tilde_fenced_code_excluded() {
    let td = tempfile::tempdir().unwrap();
    let doc = "/// p01 p02 p03 p04 p05 p06 p07 p08 p09 p10\n\
               /// ~~~\n\
               /// c01 c02 c03 c04 c05 c06 c07 c08 c09 c10\n\
               /// c11 c12 c13 c14 c15 c16 c17 c18 c19 c20\n\
               /// c21 c22 c23 c24 c25 c26 c27 c28 c29 c30\n\
               /// c31 c32 c33 c34 c35 c36 c37 c38 c39 c40\n\
               /// c41 c42 c43 c44 c45 c46 c47 c48 c49 c50\n\
               /// ~~~\n\
               pub fn f() {}\n";
    write(td.path(), "a.rs", doc);
    let out = run_lint(td.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(0),
        "tilde-fenced example body must be excluded from word budget; stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        !stdout.contains("DOC_LINT"),
        "tilde-fenced code should be excluded from word count:\n{stdout}"
    );
}
#[test]
fn lint_with_parse_error_exits_five() {
    let td = tempfile::tempdir().unwrap();
    write(td.path(), "broken.rs", "fn f( {\n");
    let over_budget = "/// w01 w02 w03 w04 w05 w06 w07 w08 w09 w10\n\
                       /// w11 w12 w13 w14 w15 w16 w17 w18 w19 w20\n\
                       /// w21 w22 w23 w24 w25 w26 w27 w28 w29 w30\n\
                       /// w31 w32 w33 w34 w35 w36 w37 w38 w39 w40\n\
                       /// w41 w42 w43 w44 w45 w46 w47 w48 w49 w50\n\
                       pub fn g() {}\n";
    write(td.path(), "wordy.rs", over_budget);
    let out = run_lint(td.path());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(5),
        "parse error during lint must exit 5:\nstderr: {stderr}"
    );
    assert!(
        stderr.contains("PARSE_ERROR"),
        "missing PARSE_ERROR diagnostic:\n{stderr}"
    );
}
#[test]
fn lint_custom_budget_honoured() {
    let td = tempfile::tempdir().unwrap();
    write(
        td.path(),
        "a.rs",
        "/// one two three four five six\npub fn f() {}\n",
    );
    let out = run_lint_budget(td.path(), 5);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(4), "stdout:\n{stdout}");
    assert!(
        stdout.contains("DOC_LINT\t") && stdout.contains("budget=5") && stdout.contains("words=6"),
        "expected DOC_LINT with budget=5 words=6:\n{stdout}"
    );
}
#[test]
fn dry_run_processes_crates_and_src_but_skips_target_and_docs() {
    let td = tempfile::tempdir().unwrap();
    let root = td.path();
    fs::create_dir_all(root.join("src")).expect("mkdir src");
    fs::write(root.join("src/lib.rs"), "// removable\nfn s() {}\n").expect("write src");
    fs::create_dir_all(root.join("crates/foo/src")).expect("mkdir crates/foo/src");
    fs::write(
        root.join("crates/foo/src/lib.rs"),
        "// removable\nfn c() {}\n",
    )
    .expect("write crates");
    fs::create_dir_all(root.join("target/package/foo-0.1.0/src")).expect("mkdir target subtree");
    fs::write(
        root.join("target/package/foo-0.1.0/src/lib.rs"),
        "// removable\nfn t() {}\n",
    )
    .expect("write target");
    fs::create_dir_all(root.join("docs")).expect("mkdir docs");
    fs::write(root.join("docs/example.rs"), "// removable\nfn d() {}\n").expect("write docs");
    fs::create_dir_all(root.join("scripts")).expect("mkdir scripts");
    fs::write(root.join("scripts/helper.rs"), "// removable\nfn h() {}\n").expect("write scripts");
    let out = run_dry(root);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "exit {:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        out.status.code()
    );
    assert!(
        stdout.contains("src/lib.rs"),
        "expected src/lib.rs WOULD_REWRITE:\n{stdout}"
    );
    assert!(
        stdout.contains("crates/foo/src/lib.rs"),
        "expected crates/foo/src/lib.rs WOULD_REWRITE:\n{stdout}"
    );
    for forbidden in ["target/package", "docs/example.rs", "scripts/helper.rs"] {
        assert!(
            !stdout.contains(forbidden),
            "unexpected out-of-scope path `{forbidden}` in dry-run output:\n{stdout}"
        );
    }
}
#[test]
fn lint_processes_crates_and_src_but_skips_target_and_docs() {
    let td = tempfile::tempdir().unwrap();
    let root = td.path();
    fs::create_dir_all(root.join("src")).expect("mkdir src");
    fs::write(
        root.join("src/lib.rs"),
        "/// one two three four five six\npub fn s() {}\n",
    )
    .expect("write src");
    fs::create_dir_all(root.join("target/package/foo-0.1.0/src")).expect("mkdir target subtree");
    fs::write(
        root.join("target/package/foo-0.1.0/src/lib.rs"),
        "/// one two three four five six\npub fn t() {}\n",
    )
    .expect("write target");
    fs::create_dir_all(root.join("docs")).expect("mkdir docs");
    fs::write(
        root.join("docs/example.rs"),
        "/// one two three four five six\npub fn d() {}\n",
    )
    .expect("write docs");
    let out = run_lint_budget(root, 5);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(4),
        "expected exit 4 (one in-scope finding); stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let lint_count = stdout.matches("DOC_LINT\t").count();
    assert_eq!(
        lint_count, 1,
        "expected exactly 1 DOC_LINT finding (src/lib.rs); got {lint_count}:\n{stdout}"
    );
    assert!(
        stdout.contains("src/lib.rs"),
        "expected DOC_LINT to cite src/lib.rs:\n{stdout}"
    );
    for forbidden in ["target/package", "docs/example.rs"] {
        assert!(
            !stdout.contains(forbidden),
            "out-of-scope path `{forbidden}` leaked into lint output:\n{stdout}"
        );
    }
}
#[test]
fn non_rust_files_under_allowed_roots_are_ignored() {
    let td = tempfile::tempdir().unwrap();
    let root = td.path();
    fs::create_dir_all(root.join("src")).expect("mkdir src");
    fs::write(root.join("src/lib.rs"), "// removable\nfn s() {}\n").expect("write rs");
    fs::write(root.join("src/script.py"), "# python\nprint('hi')\n").expect("write py");
    fs::write(root.join("src/notes.md"), "# notes\n").expect("write md");
    let out = run_dry(root);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "exit {:?}", out.status.code());
    assert!(
        stdout.contains("src/lib.rs"),
        "expected src/lib.rs in dry-run output:\n{stdout}"
    );
    for forbidden in ["script.py", "notes.md"] {
        assert!(
            !stdout.contains(forbidden),
            "non-.rs file `{forbidden}` must not be processed:\n{stdout}"
        );
    }
}
#[test]
fn root_without_crates_or_src_processes_nothing() {
    let td = tempfile::tempdir().unwrap();
    let root = td.path();
    fs::create_dir_all(root.join("docs")).expect("mkdir docs");
    fs::write(root.join("docs/example.rs"), "// removable\nfn d() {}\n").expect("write docs");
    fs::write(root.join("stray.rs"), "// removable\nfn x() {}\n").expect("write stray");
    let out = run_dry(root);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "exit {:?}\nstderr:\n{stderr}",
        out.status.code()
    );
    assert!(
        !stdout.contains("WOULD_REWRITE"),
        "no rewrites expected when root has no allowed source tree:\n{stdout}"
    );
    assert!(
        stderr.contains("rewritten=0")
            && stderr.contains("unchanged=0")
            && stderr.contains("errors=0"),
        "summary should reflect zero work:\n{stderr}"
    );
}
/// Check whether `src` survives `rustfmt --check` (i.e. cargo fmt would
/// leave it untouched). Writes to a temp file because `rustfmt --check`
/// on stdin reports diffs on stdout but still exits 0; the file-path form
/// returns non-zero exit on divergence.
fn rustfmt_check(src: &str) -> bool {
    let td = tempfile::tempdir().expect("rustfmt_check tempdir");
    let path = td.path().join("probe.rs");
    fs::write(&path, src).expect("write probe");
    let out = Command::new("rustfmt")
        .arg("--edition")
        .arg("2024")
        .arg("--check")
        .arg(&path)
        .output()
        .expect("spawn rustfmt");
    out.status.success()
}
#[test]
fn rewrite_output_passes_rustfmt_check() {
    let td = tempfile::tempdir().unwrap();
    let original = "// strip me\n\
                    use std::fmt;\n\
                    pub enum DecodeError {\n    \
                        BufferUnderflow,\n    \
                        TagOutOfRange { tag: u32 },\n    \
                        LengthOutOfRange { len: u32, max: u32 },\n    \
                        TrailingBytes,\n\
                    }\n\
                    impl fmt::Display for DecodeError {\n    \
                        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {\n        \
                            match self {\n            \
                                DecodeError::BufferUnderflow => {\n                \
                                    f.write_str(\"buffer underflow: not enough bytes\")\n            \
                                }\n            \
                                DecodeError::TagOutOfRange { tag } => write!(f, \"tag out of range: {tag}\"),\n            \
                                DecodeError::LengthOutOfRange { len, max } => {\n                \
                                    write!(f, \"length out of range: {len} exceeds max {max}\")\n            \
                                }\n            \
                                DecodeError::TrailingBytes => f.write_str(\"trailing bytes\"),\n        \
                            }\n    \
                        }\n\
                    }\n\
                    fn decode(b: u8) -> Result<DecodeError, DecodeError> {\n    \
                        match b {\n        \
                            0 => Ok(DecodeError::BufferUnderflow),\n        \
                            tag => Err(DecodeError::TagOutOfRange { tag: u32::from(tag) }),\n    \
                        }\n\
                    }\n";
    write(td.path(), "a.rs", original);
    let out = run(td.path());
    assert!(
        out.status.success(),
        "rewrite failed: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let after = read(td.path(), "a.rs");
    assert!(
        !after.contains("strip me"),
        "comment not stripped:\n{after}"
    );
    assert!(
        rustfmt_check(&after),
        "comment-free output is not rustfmt-clean:\n{after}"
    );
}
#[test]
fn dry_run_diff_target_is_rustfmt_normalized() {
    let td = tempfile::tempdir().unwrap();
    let original = "// strip me\n\
                    use std::fmt;\n\
                    pub enum DecodeError {\n    \
                        BufferUnderflow,\n    \
                        TagOutOfRange { tag: u32 },\n\
                    }\n\
                    impl fmt::Display for DecodeError {\n    \
                        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {\n        \
                            match self {\n            \
                                DecodeError::BufferUnderflow => {\n                \
                                    f.write_str(\"buffer underflow: not enough bytes\")\n            \
                                }\n            \
                                DecodeError::TagOutOfRange { tag } => write!(f, \"tag: {tag}\"),\n        \
                            }\n    \
                        }\n\
                    }\n";
    write(td.path(), "a.rs", original);
    let out = run_dry(td.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("WOULD_REWRITE"),
        "no WOULD_REWRITE:\n{stdout}"
    );
    let plus_lines: Vec<&str> = stdout
        .lines()
        .filter(|l| l.starts_with('+') && !l.starts_with("+++"))
        .collect();
    let plus_blob = plus_lines.join("\n");
    assert!(
        plus_blob.contains("BufferUnderflow => f.write_str("),
        "expected rustfmt-style single-line match arm in dry-run diff, '+' lines:\n{plus_blob}"
    );
    assert!(
        !plus_blob.contains("BufferUnderflow => {"),
        "prettyplease-style wrapped arm leaked into dry-run diff target, '+' lines:\n{plus_blob}"
    );
}
#[test]
fn root_inside_crates_subtree_is_processed_directly() {
    let td = tempfile::tempdir().unwrap();
    let root = td.path();
    let scoped = root.join("crates/foo");
    fs::create_dir_all(scoped.join("src")).expect("mkdir crates/foo/src");
    fs::write(scoped.join("src/lib.rs"), "// removable\nfn c() {}\n").expect("write");
    let out = Command::new(bin())
        .arg("--rewrite")
        .arg("--dry-run")
        .arg(&scoped)
        .output()
        .expect("failed to spawn comment-free");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "exit {:?}", out.status.code());
    assert!(
        stdout.contains("src/lib.rs"),
        "expected src/lib.rs WOULD_REWRITE when ROOT is inside crates/:\n{stdout}"
    );
}
fn run_idioms(root: &Path) -> std::process::Output {
    Command::new(bin())
        .arg("--rewrite")
        .arg("--force-dirty")
        .arg("--rustdoc-link-idioms")
        .arg(root)
        .output()
        .expect("failed to spawn comment-free")
}
fn run_idioms_dry(root: &Path) -> std::process::Output {
    Command::new(bin())
        .arg("--rewrite")
        .arg("--dry-run")
        .arg("--rustdoc-link-idioms")
        .arg(root)
        .output()
        .expect("failed to spawn comment-free")
}
#[test]
fn rustdoc_link_idioms_requires_rewrite() {
    let td = tempfile::tempdir().unwrap();
    write(td.path(), "a.rs", "fn f() {}\n");
    let out = Command::new(bin())
        .arg("--rustdoc-link-idioms")
        .arg(td.path())
        .output()
        .expect("failed to spawn comment-free");
    assert_eq!(
        out.status.code(),
        Some(2),
        "clap should require --rewrite alongside --rustdoc-link-idioms (exit 2), got {:?}",
        out.status.code()
    );
}
#[test]
fn default_rewrite_preserves_doc_link_text() {
    let td = tempfile::tempdir().unwrap();
    let original = "/// see [Type](Type) here\npub fn f() {}\n";
    write(td.path(), "a.rs", original);
    run(td.path());
    let out = read(td.path(), "a.rs");
    assert!(
        out.contains("[Type](Type)"),
        "default --rewrite must not normalise doc-link idioms, got:\n{out}"
    );
    assert!(
        !out.contains("[`Type`]"),
        "default --rewrite must not introduce ticked shortcut, got:\n{out}"
    );
}
#[test]
fn idioms_flag_collapses_redundant_explicit_link() {
    let td = tempfile::tempdir().unwrap();
    let original = "/// see [Type](Type) here\npub struct Type;\n";
    write(td.path(), "a.rs", original);
    run_idioms(td.path());
    let out = read(td.path(), "a.rs");
    assert!(
        out.contains("[`Type`]"),
        "expected ticked shortcut after collapse, got:\n{out}"
    );
    assert!(
        !out.contains("[Type](Type)"),
        "redundant explicit link survived, got:\n{out}"
    );
}
#[test]
fn idioms_flag_ticks_shortcut_when_codeish() {
    let td = tempfile::tempdir().unwrap();
    let original = "/// the [Type] applies\npub struct Type;\n";
    write(td.path(), "a.rs", original);
    run_idioms(td.path());
    let out = read(td.path(), "a.rs");
    assert!(
        out.contains("[`Type`]"),
        "expected ticked shortcut, got:\n{out}"
    );
}
#[test]
fn idioms_flag_retains_explicit_target_ticks_label() {
    let td = tempfile::tempdir().unwrap();
    let original = "/// call [begin](Self::begin) first\n\
                    pub struct S;\n\
                    impl S {\n    \
                        pub fn begin(&self) {}\n\
                    }\n";
    write(td.path(), "a.rs", original);
    run_idioms(td.path());
    let out = read(td.path(), "a.rs");
    assert!(
        out.contains("[`begin`](Self::begin)"),
        "expected label ticked, target retained, got:\n{out}"
    );
}
#[test]
fn idioms_flag_skips_fenced_code() {
    let td = tempfile::tempdir().unwrap();
    let original = "/// before\n\
                    /// ```\n\
                    /// let _: [Type] = todo!();\n\
                    /// [Type](Type)\n\
                    /// ```\n\
                    /// after\n\
                    pub struct Type;\n";
    write(td.path(), "a.rs", original);
    run_idioms(td.path());
    let out = read(td.path(), "a.rs");
    assert!(
        out.contains("let _: [Type] = todo!();"),
        "fenced [Type] must survive, got:\n{out}"
    );
    assert!(
        out.contains("[Type](Type)"),
        "fenced [Type](Type) must survive, got:\n{out}"
    );
}
#[test]
fn idioms_flag_skips_inline_code_span() {
    let td = tempfile::tempdir().unwrap();
    let original = "/// use `[Type]` syntax verbatim\npub struct Type;\n";
    write(td.path(), "a.rs", original);
    run_idioms(td.path());
    let out = read(td.path(), "a.rs");
    assert!(
        out.contains("`[Type]`"),
        "inline code span must survive, got:\n{out}"
    );
}
#[test]
fn idioms_flag_skips_url_link() {
    let td = tempfile::tempdir().unwrap();
    let original = "/// see [docs](https://example.com)\npub fn f() {}\n";
    write(td.path(), "a.rs", original);
    run_idioms(td.path());
    let out = read(td.path(), "a.rs");
    assert!(
        out.contains("[docs](https://example.com)"),
        "URL link must survive, got:\n{out}"
    );
}
#[test]
fn idioms_flag_skips_reference_style() {
    let td = tempfile::tempdir().unwrap();
    let original = "/// see [Type][ref] later\n\
                    ///\n\
                    /// [ref]: https://example.com\n\
                    pub struct Type;\n";
    write(td.path(), "a.rs", original);
    run_idioms(td.path());
    let out = read(td.path(), "a.rs");
    assert!(
        out.contains("[Type][ref]"),
        "reference-style link must survive, got:\n{out}"
    );
    assert!(
        out.contains("[ref]: https://example.com"),
        "reference definition must survive, got:\n{out}"
    );
}
#[test]
fn idioms_flag_skips_prose_label() {
    let td = tempfile::tempdir().unwrap();
    let original = "/// see [the writer](Writer) for\npub struct Writer;\n";
    write(td.path(), "a.rs", original);
    run_idioms(td.path());
    let out = read(td.path(), "a.rs");
    assert!(
        out.contains("[the writer](Writer)"),
        "prose label must not be rewritten, got:\n{out}"
    );
}
#[test]
fn idioms_flag_dry_run_does_not_modify_file() {
    let td = tempfile::tempdir().unwrap();
    let original = "/// see [Type](Type)\npub struct Type;\n";
    write(td.path(), "a.rs", original);
    let out = run_idioms_dry(td.path());
    let after = read(td.path(), "a.rs");
    assert_eq!(after, original, "dry-run must not write");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("WOULD_REWRITE") && stdout.contains("[`Type`]"),
        "dry-run diff should show the would-be rewrite, got stdout:\n{stdout}"
    );
}
#[test]
fn idioms_flag_preserves_fence_state_across_doc_lines() {
    let td = tempfile::tempdir().unwrap();
    let original = "/// before [A]\n\
                    /// ```\n\
                    /// [B](B)\n\
                    /// ```\n\
                    /// after [C]\n\
                    pub fn f() {}\n";
    write(td.path(), "a.rs", original);
    run_idioms(td.path());
    let out = read(td.path(), "a.rs");
    assert!(
        out.contains("[`A`]"),
        "[A] outside fence should tick:\n{out}"
    );
    assert!(
        out.contains("[B](B)"),
        "[B](B) inside fence must survive:\n{out}"
    );
    assert!(
        out.contains("[`C`]"),
        "[C] outside fence should tick:\n{out}"
    );
}
#[test]
fn safe_idiom_path_does_not_treat_marker_in_string_literal_as_marker() {
    let td = tempfile::tempdir().unwrap();
    let original = r#"pub const BEGIN_MARKER: &str = "// AUTO-TRAIT-POLICY-BEGIN";
pub const END_MARKER: &str = "// AUTO-TRAIT-POLICY-END";
pub fn f() {}
"#;
    write(td.path(), "a.rs", original);
    run_idioms(td.path());
    let out = read(td.path(), "a.rs");
    assert_eq!(
        out, original,
        "marker text inside string literal must not be treated as a real marker region; safe idiom path must preserve every non-doc byte. got:\n{out}"
    );
}
#[test]
fn safe_idiom_path_does_not_corrupt_fixture_with_marker_and_anchor_in_string_literal() {
    let td = tempfile::tempdir().unwrap();
    let original = "fn build_fixture() -> &'static str {\n    \
                    \"// AUTO-TRAIT-POLICY-BEGIN\\n\\\n                     \
                    assert_auto_traits! {\\n    \\\n                         \
                        SendSync { Foo }\\n\\\n                     \
                    }\\n\\\n                     \
                    // AUTO-TRAIT-POLICY-END\\n\"\n\
                    }\n\
                    pub fn caller() { let _ = build_fixture(); }\n";
    write(td.path(), "a.rs", original);
    run_idioms(td.path());
    let out = read(td.path(), "a.rs");
    assert_eq!(
        out, original,
        "marker text + assert_auto_traits! anchor *both* inside a string literal must round-trip byte-identical under safe idiom path (this is exactly the end_to_end.rs corruption class). got:\n{out}"
    );
}
#[test]
fn safe_idiom_path_preserves_quote_macro_invocation_byte_identical() {
    let td = tempfile::tempdir().unwrap();
    let original = "fn build_tokens() -> proc_macro2::TokenStream {\n    \
                    let metas: proc_macro2::TokenStream = Default::default();\n    \
                    quote::quote!(#metas)\n\
                    }\n";
    write(td.path(), "a.rs", original);
    run_idioms(td.path());
    let out = read(td.path(), "a.rs");
    assert_eq!(
        out, original,
        "quote!(#metas) outside any doc comment must round-trip byte-identical under safe idiom path. got:\n{out}"
    );
}
#[test]
fn safe_idiom_path_preserves_preserved_markers_const_byte_identical() {
    let td = tempfile::tempdir().unwrap();
    let original = "pub struct PreservedMarkerPair {\n    \
                    pub begin_token: &'static str,\n    \
                    pub end_token: &'static str,\n    \
                    pub anchor_macro: &'static str,\n\
                    }\n\
                    pub const DEFAULT_PRESERVED_MARKERS: &[PreservedMarkerPair] = &[PreservedMarkerPair {\n    \
                        begin_token: \"AUTO-TRAIT-POLICY-BEGIN\",\n    \
                        end_token: \"AUTO-TRAIT-POLICY-END\",\n    \
                        anchor_macro: \"assert_auto_traits\",\n\
                    }];\n";
    write(td.path(), "a.rs", original);
    run_idioms(td.path());
    let out = read(td.path(), "a.rs");
    assert_eq!(
        out, original,
        "DEFAULT_PRESERVED_MARKERS struct literal must round-trip byte-identical under safe idiom path. got:\n{out}"
    );
}
#[test]
fn safe_idiom_path_preserves_non_doc_bytes_when_no_doc_changes() {
    let td = tempfile::tempdir().unwrap();
    let original = "use pardosa::store::{Event, FiberId};\n\
                    use pardosa::store::{ExtractError, FiberIndex, FiberLookup};\n\
                    fn _names_used() {\n    \
                        let _: FiberIndex<u64> = FiberIndex::empty();\n    \
                        let _: FiberLookup<FiberId> = FiberLookup::Empty;\n\
                    }\n";
    write(td.path(), "a.rs", original);
    run_idioms(td.path());
    let out = read(td.path(), "a.rs");
    assert_eq!(
        out, original,
        "safe idiom path must not touch non-doc bytes when no doc-link idioms are present; got:\n{out}"
    );
}
#[test]
fn safe_idiom_path_rewrites_outer_line_doc() {
    let td = tempfile::tempdir().unwrap();
    let original = "/// see [Type](Type) here\npub struct Type;\n";
    write(td.path(), "a.rs", original);
    run_idioms(td.path());
    let out = read(td.path(), "a.rs");
    assert_eq!(
        out, "/// see [`Type`] here\npub struct Type;\n",
        "outer /// doc-link idiom must be rewritten; got:\n{out}"
    );
}
#[test]
fn safe_idiom_path_rewrites_inner_line_doc() {
    let td = tempfile::tempdir().unwrap();
    let original = "//! crate-level [Type](Type) doc\npub struct Type;\n";
    write(td.path(), "a.rs", original);
    run_idioms(td.path());
    let out = read(td.path(), "a.rs");
    assert_eq!(
        out, "//! crate-level [`Type`] doc\npub struct Type;\n",
        "inner //! doc-link idiom must be rewritten; got:\n{out}"
    );
}
#[test]
fn safe_idiom_path_rewrites_explicit_doc_attr() {
    let td = tempfile::tempdir().unwrap();
    let original = "#[doc = \" see [Type](Type) here\"]\npub struct Type;\n";
    write(td.path(), "a.rs", original);
    run_idioms(td.path());
    let out = read(td.path(), "a.rs");
    assert_eq!(
        out, "#[doc = \" see [`Type`] here\"]\npub struct Type;\n",
        "#[doc=\"...\"] doc-link idiom must be rewritten; got:\n{out}"
    );
}
#[test]
fn safe_idiom_path_rewrites_cfg_attr_doc() {
    let td = tempfile::tempdir().unwrap();
    let original = "#[cfg_attr(test, doc = \" see [Type](Type) here\")]\npub struct Type;\n";
    write(td.path(), "a.rs", original);
    run_idioms(td.path());
    let out = read(td.path(), "a.rs");
    assert_eq!(
        out, "#[cfg_attr(test, doc = \" see [`Type`] here\")]\npub struct Type;\n",
        "cfg_attr(_, doc=\"...\") doc-link idiom must be rewritten; got:\n{out}"
    );
}
#[test]
fn safe_idiom_path_is_idempotent() {
    let td = tempfile::tempdir().unwrap();
    let original = "/// see [Type](Type) and [`Other`]\npub struct Type;\npub struct Other;\n";
    write(td.path(), "a.rs", original);
    run_idioms(td.path());
    let pass1 = read(td.path(), "a.rs");
    run_idioms(td.path());
    let pass2 = read(td.path(), "a.rs");
    assert_eq!(
        pass1, pass2,
        "safe idiom path must be idempotent; pass1:\n{pass1}\npass2:\n{pass2}"
    );
}
#[test]
fn safe_idiom_path_preserves_line_count() {
    let td = tempfile::tempdir().unwrap();
    let original = "/// summary line 1\n\
                    /// see [Type](Type) here\n\
                    ///\n\
                    /// # Errors\n\
                    ///\n\
                    /// none\n\
                    pub struct Type;\n\
                    fn helper() {\n    \
                        let _ = 1;\n\
                    }\n";
    let lines_before = original.matches('\n').count();
    write(td.path(), "a.rs", original);
    run_idioms(td.path());
    let out = read(td.path(), "a.rs");
    let lines_after = out.matches('\n').count();
    assert_eq!(
        lines_before, lines_after,
        "safe idiom path must preserve line count; before={lines_before}, after={lines_after}\n--- BEFORE ---\n{original}--- AFTER ---\n{out}"
    );
}
#[test]
fn safe_idiom_path_preserves_block_doc_comment_unchanged() {
    let td = tempfile::tempdir().unwrap();
    let original = "/** see [Type](Type) here */\npub struct Type;\n";
    write(td.path(), "a.rs", original);
    run_idioms(td.path());
    let out = read(td.path(), "a.rs");
    assert!(
        out.starts_with("/**"),
        "block /** ... */ doc must be left textually as a block doc by the safe path (no AST round-trip); got:\n{out}"
    );
    assert!(
        out.contains("[Type](Type)") || out.contains("[`Type`]"),
        "block doc payload should either be left verbatim (preferred) or be rewritten in place — but it must not be deleted; got:\n{out}"
    );
}
#[test]
fn safe_idiom_path_dry_run_emits_only_doc_line_changes() {
    let td = tempfile::tempdir().unwrap();
    let original = "use std::collections::HashMap;\n\
                    use std::collections::BTreeMap;\n\
                    /// see [Type](Type)\n\
                    pub struct Type;\n\
                    fn helper(m: HashMap<u32, u32>, b: BTreeMap<u32, u32>) -> usize {\n    \
                        m.len() + b.len()\n\
                    }\n";
    write(td.path(), "a.rs", original);
    let out = run_idioms_dry(td.path());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let plus_minus: Vec<&str> = stdout
        .lines()
        .filter(|l| {
            (l.starts_with('+') && !l.starts_with("+++"))
                || (l.starts_with('-') && !l.starts_with("---"))
        })
        .collect();
    for line in &plus_minus {
        let body = &line[1..];
        let trimmed = body.trim_start();
        assert!(
            trimmed.starts_with("///")
                || trimmed.starts_with("//!")
                || trimmed.starts_with("#[doc")
                || trimmed.starts_with("#![doc")
                || trimmed.starts_with("#[cfg_attr"),
            "diff line outside doc surface in safe idiom path: {line:?}\nfull stdout:\n{stdout}"
        );
    }
}
#[test]
fn safe_idiom_path_does_not_inject_auto_trait_markers() {
    let td = tempfile::tempdir().unwrap();
    let original = "/// see [Type](Type)\npub struct Type;\n";
    write(td.path(), "a.rs", original);
    run_idioms(td.path());
    let out = read(td.path(), "a.rs");
    assert!(
        !out.contains("AUTO-TRAIT-POLICY"),
        "safe idiom path must not invoke marker restoration logic; got:\n{out}"
    );
}
