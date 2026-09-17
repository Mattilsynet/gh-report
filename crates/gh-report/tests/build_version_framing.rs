use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const FIXTURE_PACKAGE_VERSION: &str = "9.9.9";
const SENTINEL: &str = "GHR_FRAME_PROBE";
const LF_PAYLOAD: &str = "v1.2.3\ncargo:rustc-env=GHR_FRAME_PROBE=1";
const CRLF_PAYLOAD: &str = "v1.2.3\r\ncargo:rustc-env=GHR_FRAME_PROBE=1";
const CR_PAYLOAD: &str = "v1.2.3\rcargo:rustc-env=GHR_FRAME_PROBE=1";

#[test]
fn lf_payload_cannot_inject_a_cargo_record() {
    let probe = Fixture::new("lf").app_version(LF_PAYLOAD).run();
    probe.assert_uncontaminated();
    assert_eq!(probe.version, FIXTURE_PACKAGE_VERSION);
    probe.assert_rejection_warning("APP_VERSION", "contains a record separator");
}

#[test]
fn crlf_payload_cannot_inject_a_cargo_record() {
    let probe = Fixture::new("crlf").app_version(CRLF_PAYLOAD).run();
    probe.assert_uncontaminated();
    assert_eq!(probe.version, FIXTURE_PACKAGE_VERSION);
    probe.assert_rejection_warning("APP_VERSION", "contains a record separator");
}

#[test]
fn cr_payload_cannot_inject_a_cargo_record() {
    let probe = Fixture::new("cr").app_version(CR_PAYLOAD).run();
    probe.assert_uncontaminated();
    assert_eq!(probe.version, FIXTURE_PACKAGE_VERSION);
    probe.assert_rejection_warning("APP_VERSION", "contains a record separator");
}

#[test]
fn valid_candidate_is_still_stamped_without_any_warning() {
    let probe = Fixture::new("valid")
        .app_version("v1.2.3-2-g4549a1f-dirty")
        .run();
    probe.assert_uncontaminated();
    assert_eq!(probe.version, "1.2.3-2-g4549a1f-dirty");
    assert!(
        !probe.stderr.contains("gh-report build: rejected"),
        "a legitimate candidate must emit no rejection warning: {}",
        probe.stderr
    );
}

#[test]
fn valid_override_wins_over_available_git_describe() {
    let probe = Fixture::new("precedence")
        .with_git_tag("v1.9.9")
        .app_version("v2.0.0")
        .run();
    probe.assert_uncontaminated();
    assert_eq!(probe.version, "2.0.0");
}

#[test]
fn rejected_override_falls_through_to_git_describe() {
    let probe = Fixture::new("override-to-git")
        .with_git_tag("v1.9.9")
        .app_version(LF_PAYLOAD)
        .run();
    probe.assert_uncontaminated();
    assert_eq!(probe.version, "1.9.9");
    probe.assert_rejection_warning("APP_VERSION", "contains a record separator");
}

#[test]
fn v_only_override_falls_through_to_git_describe() {
    let probe = Fixture::new("v-only-to-git")
        .with_git_tag("v1.9.9")
        .app_version("v")
        .run();
    probe.assert_uncontaminated();
    assert_eq!(probe.version, "1.9.9");
    probe.assert_rejection_warning("APP_VERSION", "is empty after version prefix normalization");
}

#[test]
fn v_only_override_without_git_falls_through_to_package() {
    let probe = Fixture::new("v-only-to-package").app_version("v").run();
    probe.assert_uncontaminated();
    assert_eq!(probe.version, FIXTURE_PACKAGE_VERSION);
    probe.assert_rejection_warning("APP_VERSION", "is empty after version prefix normalization");
}

#[test]
fn empty_normalized_git_candidate_falls_through_to_package() {
    let probe = Fixture::new("git-to-package").with_git_tag("v").run();
    probe.assert_uncontaminated();
    assert_eq!(probe.version, FIXTURE_PACKAGE_VERSION);
    probe.assert_rejection_warning(
        "git-describe",
        "is empty after version prefix normalization",
    );
}

#[test]
fn contaminated_parent_environment_does_not_fake_a_regression() {
    let probe = Fixture::new("ambient")
        .app_version(LF_PAYLOAD)
        .contaminate_parent_environment()
        .run();
    probe.assert_uncontaminated();
    assert_eq!(probe.version, FIXTURE_PACKAGE_VERSION);
}

#[test]
fn contaminated_parent_environment_does_not_divert_fixture_git_setup() {
    let probe = Fixture::new("ambient-git")
        .with_git_tag("v1.9.9")
        .app_version(LF_PAYLOAD)
        .contaminate_parent_environment()
        .run();
    probe.assert_uncontaminated();
    assert_eq!(probe.version, "1.9.9");
}

#[test]
fn caller_boundary_cannot_construct_or_mutate_the_validated_version() {
    for bypass in [
        "    let _bypass = EmittableVersion(String::from(\"\\ncargo:rustc-env=GHR_FRAME_PROBE=1\"));",
        "    let mut _bypass = EmittableVersion::new(\"1.2.3\").unwrap();\n    _bypass.0 = String::from(\"\\ncargo:rustc-env=GHR_FRAME_PROBE=1\");",
    ] {
        let failure = Fixture::new("sealed")
            .app_version("v1.2.3")
            .inject_into_build_main(bypass)
            .run_expecting_failure();
        assert!(
            failure.contains("E0603") || failure.contains("private"),
            "the validated type must resist caller-boundary construction/mutation: {failure}"
        );
    }
}

struct Fixture {
    name: String,
    app_version: String,
    git_tag: Option<String>,
    injected_main_statement: Option<String>,
    contaminate: bool,
}

struct FixtureProbe {
    version: String,
    probe: String,
    stderr: String,
}

impl FixtureProbe {
    fn assert_uncontaminated(&self) {
        assert_eq!(
            self.probe, "unset",
            "a rejected or legitimate candidate must never set the sentinel"
        );
    }

    fn assert_rejection_warning(&self, source: &str, reason: &str) {
        let expected = format!("gh-report build: rejected {source} version candidate: {reason}");
        assert!(
            self.stderr.contains(&expected),
            "expected the fixed rejection warning {expected:?} in build stderr: {}",
            self.stderr
        );
        for raw in [LF_PAYLOAD, CRLF_PAYLOAD, CR_PAYLOAD] {
            let candidate_tail = raw.trim_start_matches("v1.2.3").trim();
            assert!(
                !self.stderr.contains(candidate_tail),
                "the warning must never reproduce raw candidate text: {}",
                self.stderr
            );
        }
    }
}

impl Fixture {
    fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            app_version: String::new(),
            git_tag: None,
            injected_main_statement: None,
            contaminate: false,
        }
    }

    fn app_version(mut self, value: &str) -> Self {
        self.app_version = value.to_string();
        self
    }

    fn with_git_tag(mut self, tag: &str) -> Self {
        self.git_tag = Some(tag.to_string());
        self
    }

    fn inject_into_build_main(mut self, statement: &str) -> Self {
        self.injected_main_statement = Some(statement.to_string());
        self
    }

    fn contaminate_parent_environment(mut self) -> Self {
        self.contaminate = true;
        self
    }

    fn run(self) -> FixtureProbe {
        let (output, _temp) = self.execute();
        assert!(
            output.status.success(),
            "fixture build failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8(output.stdout).expect("fixture stdout is utf-8");
        let mut probe = FixtureProbe {
            version: String::new(),
            probe: String::new(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        };
        for line in stdout.lines() {
            if let Some(value) = line.strip_prefix("version=") {
                probe.version = value.to_string();
            } else if let Some(value) = line.strip_prefix("probe=") {
                probe.probe = value.to_string();
            }
        }
        probe
    }

    fn run_expecting_failure(self) -> String {
        let (output, _temp) = self.execute();
        assert!(
            !output.status.success(),
            "the fixture was expected to fail compilation"
        );
        String::from_utf8_lossy(&output.stderr).into_owned()
    }

    fn execute(self) -> (std::process::Output, tempfile::TempDir) {
        let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join(&self.name);
        fs::create_dir_all(root.join("src")).expect("create fixture src");

        fs::copy(manifest_dir.join("build_env.rs"), root.join("build_env.rs"))
            .expect("copy build_env.rs");
        write_build_script(manifest_dir, &root, self.injected_main_statement.as_deref());

        fs::write(
            root.join("Cargo.toml"),
            format!(
                "[package]\nname = \"framing-fixture\"\nversion = \"{FIXTURE_PACKAGE_VERSION}\"\n\
                 edition = \"2024\"\n\n[workspace]\n"
            ),
        )
        .expect("write fixture manifest");

        fs::write(
            root.join("src/main.rs"),
            "fn main() {\n    println!(\"version={}\", env!(\"GH_REPORT_VERSION\"));\n    \
             println!(\"probe={}\", option_env!(\"GHR_FRAME_PROBE\").unwrap_or(\"unset\"));\n}\n",
        )
        .expect("write fixture main");

        match &self.git_tag {
            Some(tag) => init_git_repository(&root, tag, self.contaminate),
            None => assert!(
                !root.join(".git").exists(),
                "the no-git cases must not see a repository"
            ),
        }

        let mut command = Command::new(env!("CARGO"));
        command.current_dir(&root).args(["run", "--offline"]);
        if self.contaminate {
            command.env(SENTINEL, "ambient");
            command.env("GIT_DIR", root.join("absent-git-dir"));
        }
        command
            .env("APP_VERSION", &self.app_version)
            .env("CARGO_TARGET_DIR", temp.path().join("target"))
            .env("CARGO_TERM_PROGRESS_WHEN", "never")
            .env_remove(SENTINEL)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_COMMON_DIR")
            .env_remove("RUSTC_WORKSPACE_WRAPPER")
            .env_remove("RUSTC_WRAPPER");

        let output = command.output().expect("run fixture cargo");
        (output, temp)
    }
}

fn write_build_script(manifest_dir: &Path, root: &Path, injected: Option<&str>) {
    let source = fs::read_to_string(manifest_dir.join("build.rs")).expect("read build.rs");
    let script = match injected {
        None => source,
        Some(statement) => {
            let anchor = "fn main() {\n";
            let at = source.find(anchor).expect("build.rs declares fn main");
            let (head, tail) = source.split_at(at + anchor.len());
            format!("{head}{statement}\n{tail}")
        }
    };
    fs::write(root.join("build.rs"), script).expect("write fixture build.rs");
}

fn init_git_repository(root: &PathBuf, tag: &str, contaminated_parent: bool) {
    let git = |args: &[&str]| {
        let mut command = Command::new("git");
        command.current_dir(root);
        if contaminated_parent {
            command.env("GIT_DIR", root.join("absent-git-dir"));
            command.env("GIT_WORK_TREE", root.join("absent-work-tree"));
            command.env("GIT_INDEX_FILE", root.join("absent-index"));
            command.env("GIT_COMMON_DIR", root.join("absent-common-dir"));
        }
        let status = command
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_COMMON_DIR")
            .args(args)
            .output()
            .expect("run git in fixture");
        assert!(
            status.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&status.stderr)
        );
    };
    git(&["init", "--quiet", "--initial-branch=main"]);
    git(&["config", "user.email", "fixture@example.invalid"]);
    git(&["config", "user.name", "fixture"]);
    git(&["config", "commit.gpgsign", "false"]);
    git(&["add", "."]);
    git(&["commit", "--quiet", "-m", "fixture"]);
    git(&["tag", tag]);
}

mod build_script_protocol {
    include!(concat!(env!("CARGO_MANIFEST_DIR"), "/build_env.rs"));

    #[test]
    fn emittable_version_accepts_legitimate_spellings_and_strips_exactly_one_v() {
        let cases = [
            ("v0.1.83", "0.1.83"),
            ("0.1.83", "0.1.83"),
            ("vv1", "v1"),
            ("pr-77", "pr-77"),
            ("pr-4549a1f", "pr-4549a1f"),
            ("v0.1.83-2-g4549a1f-dirty", "0.1.83-2-g4549a1f-dirty"),
            ("  spaced  ", "  spaced  "),
        ];
        for (raw, expected) in cases {
            let accepted = EmittableVersion::new(raw)
                .unwrap_or_else(|_| panic!("legitimate spelling must be accepted: {raw:?}"));
            assert_eq!(accepted.as_str(), expected);
        }
    }

    #[test]
    fn emittable_version_rejects_record_separators_and_empty_after_prefix() {
        for raw in [
            "v1.2.3\ncargo:rustc-env=GHR_FRAME_PROBE=1",
            "v1.2.3\r\ncargo:rustc-env=GHR_FRAME_PROBE=1",
            "v1.2.3\rcargo:rustc-env=GHR_FRAME_PROBE=1",
        ] {
            assert_eq!(
                EmittableVersion::new(raw).unwrap_err(),
                build_version::VersionRejection::RecordSeparator,
                "record separator must not reach the Cargo protocol: {raw:?}"
            );
        }
        for raw in ["v", ""] {
            assert_eq!(
                EmittableVersion::new(raw).unwrap_err(),
                build_version::VersionRejection::EmptyAfterPrefix
            );
        }
    }

    #[test]
    fn package_fallback_is_a_validated_version() {
        let fallback = EmittableVersion::package_fallback();
        assert_eq!(fallback.as_str(), env!("CARGO_PKG_VERSION"));
        assert!(EmittableVersion::new(fallback.as_str()).is_ok());
    }

    #[test]
    fn version_rejection_diagnostic_is_fixed_and_never_echoes_the_candidate() {
        let diagnostic = version_rejection_diagnostic(
            "APP_VERSION",
            build_version::VersionRejection::RecordSeparator,
        );
        assert_eq!(
            diagnostic,
            "cargo:warning=gh-report build: rejected APP_VERSION version candidate: contains a record separator"
        );
        assert!(!diagnostic.contains('\n'));
        assert!(!diagnostic.contains('\r'));
    }

    #[test]
    fn sanitize_git_sha_rejects_multiline_and_malformed_input() {
        let rejected = [
            "",
            "   ",
            "not-a-sha",
            "21b32e",
            &"a".repeat(41),
            "abcdef0\ncargo:warning=INJECTED",
            "abcdef0\rcargo:rustc-env=GH_REPORT_VERSION=injected",
            "abcdef0\ncargo:rustc-env=GH_REPORT_GIT_SHA=deadbee",
        ];
        for raw in rejected {
            assert_eq!(
                sanitize_git_sha(raw),
                "",
                "malformed build input must not reach the Cargo protocol: {raw:?}"
            );
        }
    }

    #[test]
    fn sanitize_git_sha_preserves_provisioned_hex_without_extra_records() {
        for raw in [
            "21b32e3",
            "  21b32e3  ",
            "21B32E3DEADBEEF0123456789ABCDEF012345678",
        ] {
            let emitted = format!(
                "cargo:rustc-env=GH_REPORT_GIT_SHA={}",
                sanitize_git_sha(raw)
            );
            assert_eq!(
                emitted.lines().count(),
                1,
                "emitted directive must stay a single Cargo record: {raw:?}"
            );
            assert_eq!(
                emitted.matches("cargo:").count(),
                1,
                "emitted directive must not introduce a second Cargo record: {raw:?}"
            );
        }
        assert_eq!(sanitize_git_sha("  21b32e3  "), "21b32e3");
    }
}
