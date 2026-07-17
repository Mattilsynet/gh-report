/// File-permission preflight for the NATS creds file (SEC-0007
/// confidentiality intent, mission `adr-fmt-1a060`). Warns rather
/// than errors by default; a permissive mode is not itself proof of
/// compromise, only of unnecessary exposure.
#[must_use]
pub fn creds_mode_is_permissive(mode: u32) -> bool {
    mode & 0o077 != 0
}

#[cfg(unix)]
pub fn warn_if_creds_permissive(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let Ok(metadata) = std::fs::metadata(path) else {
        return;
    };
    let mode = metadata.permissions().mode();
    if creds_mode_is_permissive(mode) {
        eprintln!(
            "warning: NATS creds file {} is group/world-readable (mode {:o}); consider chmod 600",
            path.display(),
            mode & 0o777
        );
    }
}

#[cfg(not(unix))]
pub fn warn_if_creds_permissive(_path: &std::path::Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_0600_not_permissive() {
        assert!(!creds_mode_is_permissive(0o600));
    }

    #[test]
    fn mode_0644_permissive() {
        assert!(creds_mode_is_permissive(0o644));
    }

    #[test]
    fn mode_0640_permissive() {
        assert!(creds_mode_is_permissive(0o640));
    }

    #[test]
    fn mode_0604_permissive() {
        assert!(creds_mode_is_permissive(0o604));
    }

    #[test]
    fn mode_0700_not_permissive() {
        assert!(!creds_mode_is_permissive(0o700));
    }
}
