fn sanitize_git_sha(raw: &str) -> &str {
    let candidate = raw.trim();
    let provisioned = (7..=40).contains(&candidate.len())
        && candidate.bytes().all(|b| b.is_ascii_hexdigit());
    if provisioned { candidate } else { "" }
}

mod build_version {
    const UNRESOLVED_PACKAGE_VERSION: &str = "0.0.0";

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum VersionRejection {
        RecordSeparator,
        EmptyAfterPrefix,
    }

    impl VersionRejection {
        pub fn reason(self) -> &'static str {
            match self {
                Self::RecordSeparator => "contains a record separator",
                Self::EmptyAfterPrefix => "is empty after version prefix normalization",
            }
        }
    }

    #[derive(Debug)]
    pub struct EmittableVersion(String);

    impl EmittableVersion {
        pub fn new(candidate: &str) -> Result<Self, VersionRejection> {
            let normalized = candidate.strip_prefix('v').unwrap_or(candidate);
            match normalized.bytes().find(|b| matches!(b, b'\n' | b'\r')) {
                Some(_) => Err(VersionRejection::RecordSeparator),
                None if normalized.is_empty() => Err(VersionRejection::EmptyAfterPrefix),
                None => Ok(Self(normalized.to_string())),
            }
        }

        pub fn package_fallback() -> Self {
            match Self::new(env!("CARGO_PKG_VERSION")) {
                Ok(validated) => validated,
                Err(_) => Self(UNRESOLVED_PACKAGE_VERSION.to_string()),
            }
        }

        pub fn as_str(&self) -> &str {
            &self.0
        }
    }

    pub fn version_rejection_diagnostic(source: &str, rejection: VersionRejection) -> String {
        format!(
            "cargo:warning=gh-report build: rejected {source} version candidate: {}",
            rejection.reason()
        )
    }
}

use build_version::{EmittableVersion, version_rejection_diagnostic};
