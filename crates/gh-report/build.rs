use std::env;
use std::process::Command;

include!("build_env.rs");

fn main() {
    let version = resolve_version();
    println!("cargo:rustc-env=GH_REPORT_VERSION={version}");
    println!("cargo:rerun-if-env-changed=APP_VERSION");

    let raw_git_sha = env::var("APP_GIT_SHA").unwrap_or_default();
    let git_sha = sanitize_git_sha(&raw_git_sha);
    println!("cargo:rustc-env=GH_REPORT_GIT_SHA={git_sha}");
    println!("cargo:rerun-if-env-changed=APP_GIT_SHA");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=build_env.rs");
}

fn resolve_version() -> String {
    if let Ok(app_version) = env::var("APP_VERSION")
        && !app_version.is_empty()
    {
        return strip_v_prefix(&app_version).to_string();
    }
    if let Some(described) = git_describe() {
        return strip_v_prefix(&described).to_string();
    }
    env!("CARGO_PKG_VERSION").to_string()
}

fn git_describe() -> Option<String> {
    let result = Command::new("git")
        .args(["describe", "--tags", "--always", "--dirty"])
        .output();
    match result {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let trimmed = stdout.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
        _ => None,
    }
}

fn strip_v_prefix(s: &str) -> &str {
    s.strip_prefix('v').unwrap_or(s)
}
