use std::env;
use std::process::Command;

fn main() {
    let version = resolve_version();
    println!("cargo:rustc-env=GH_REPORT_VERSION={version}");
    println!("cargo:rerun-if-env-changed=APP_VERSION");
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
