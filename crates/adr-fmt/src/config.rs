//! Configuration loading from `adr-fmt.toml`.
//!
//! The config file lives at the workspace root and is the SSOT discovery
//! marker. It defines the corpus root, domain mappings, stale directory,
//! and optional rule parameter overrides. Rules themselves are hardcoded
//! in the binary. Rationale and judgment guidance live in dedicated ADRs
//! under `docs/adr/adr-fmt/` (see AFM-0001, AFM-0020).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Top-level configuration.
#[derive(Debug, Deserialize)]
pub struct Config {
    pub corpus: CorpusConfig,
    pub stale: StaleConfig,
    pub domains: Vec<DomainConfig>,
    /// Optional rule overrides. If present with full declarations (legacy
    /// format), a deprecation warning is emitted to stderr.
    #[serde(default)]
    pub rules: Vec<RuleConfig>,
}

/// Corpus-root configuration. The `root` value is a relative path from
/// the directory containing `adr-fmt.toml` to the ADR corpus directory.
#[derive(Debug, Deserialize)]
pub struct CorpusConfig {
    pub root: String,
}

/// Stale archive configuration.
#[derive(Debug, Deserialize)]
pub struct StaleConfig {
    pub directory: String,
}

/// Domain definition.
#[derive(Debug, Deserialize)]
pub struct DomainConfig {
    pub prefix: String,
    pub name: String,
    pub directory: String,
    pub description: String,
    pub crates: Vec<String>,
    /// Foundation domains are included with every domain query.
    #[serde(default)]
    pub foundation: bool,
    /// Rationale for having more than one Root ADR in this domain.
    ///
    /// Per the parent-edge tree model (AFM-0020), every domain is
    /// expected to have exactly one Root ADR. A multi-root domain is
    /// permitted only when the domain genuinely splits into independent
    /// concerns; in that case this field documents why.
    ///
    /// **Status: parsed but inert.** The accompanying warning ("emit
    /// when domain has >1 root and rationale is empty") is not yet
    /// wired. Tracked as a follow-up to the parent-edge migration.
    #[serde(default)]
    pub multi_root_rationale: String,
}

/// Rule override entry. Only `id` is required; other fields are optional
/// and used only for parameter overrides or disabling rules.
#[derive(Debug, Deserialize)]
pub struct RuleConfig {
    pub id: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub description: String,
    /// Optional rule parameters (e.g., `min_words = 7`).
    #[serde(default)]
    pub params: HashMap<String, toml::Value>,
}

impl Config {
    /// Look up a rule parameter by rule ID and key.
    ///
    /// Returns `None` if the rule or key does not exist.
    pub fn rule_param_u64(&self, rule_id: &str, key: &str) -> Option<u64> {
        self.rules
            .iter()
            .find(|r| r.id == rule_id)
            .and_then(|r| r.params.get(key))
            .and_then(toml::Value::as_integer)
            .and_then(|v| u64::try_from(v).ok())
    }
}

/// Load configuration from `adr-fmt.toml` in the marker directory,
/// suppressing the legacy-rule deprecation warning.
///
/// `marker_dir` is the directory containing `adr-fmt.toml` (typically
/// the workspace root). Used by walk-up discovery so warnings from
/// skipped (non-selected) markers do not pollute stderr.
///
/// # Errors
///
/// Returns [`LoadError::Io`] when `adr-fmt.toml` cannot be read.
/// Returns [`LoadError::Parse`] when TOML parsing fails or the required
/// `[corpus]` table is absent.
pub fn load_quiet(marker_dir: &Path) -> Result<Config, LoadError> {
    load_inner_typed(marker_dir)
}

/// Distinguishes how a marker load failed. `Io` indicates the file
/// existed but could not be read (permission denied, etc.) — discovery
/// should treat this as a hard error rather than skip. `Parse` covers
/// malformed TOML or a missing `[corpus]` table — discovery may skip
/// and continue walking.
#[derive(Debug)]
#[non_exhaustive]
pub enum LoadError {
    Io(String),
    Parse(String),
}

impl core::fmt::Display for LoadError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            LoadError::Io(msg) => write!(f, "adr-fmt config I/O error: {msg}"),
            LoadError::Parse(msg) => write!(f, "adr-fmt config parse error: {msg}"),
        }
    }
}

impl std::error::Error for LoadError {}

fn load_inner_typed(marker_dir: &Path) -> Result<Config, LoadError> {
    let config_path = marker_dir.join("adr-fmt.toml");

    let content = std::fs::read_to_string(&config_path).map_err(|e| {
        LoadError::Io(format!(
            "cannot read {}: {e}\n       adr-fmt.toml is required at the workspace root",
            config_path.display()
        ))
    })?;

    let config: Config = toml::from_str(&content).map_err(|e| {
        let msg = e.to_string();
        if msg.contains("missing field `corpus`") {
            LoadError::Parse(format!(
                "{}: missing required `[corpus]` table\n\
                 \n\
                 Example:\n\
                 \n\
                     [corpus]\n\
                     root = \"docs/adr\"\n",
                config_path.display()
            ))
        } else {
            LoadError::Parse(format!("failed to parse {}: {e}", config_path.display()))
        }
    })?;

    Ok(config)
}

/// Resolve the corpus root path relative to the marker directory.
///
/// Applies strict containment via [`crate::containment::contained_join`]:
/// the configured `corpus.root` must be a relative path with no parent-
/// traversal components, and the canonical target must be a descendant
/// of the canonical marker directory. The corpus directory must exist.
///
/// # Errors
///
/// Returns an error when `corpus.root` fails containment validation,
/// canonicalization, or descendant checks.
pub fn resolve_corpus_root(marker_dir: &Path, corpus: &CorpusConfig) -> Result<PathBuf, String> {
    crate::containment::contained_join(marker_dir, &corpus.root)
        .map_err(|e| format!("[corpus] root: {e}"))
}

/// Emit deprecation warnings if config contains legacy full rule declarations.
///
/// Legacy format: rules with `category` and `description` fields populated.
/// New format: only `id` and optional `params` for overrides.
///
/// Public so `main.rs` can fire it once on the *selected* marker after
/// walk-up discovery — the walk-up itself uses [`load_quiet`], which
/// suppresses the warning for skipped (non-selected) markers so stderr
/// stays focused on the marker the user actually committed to.
pub fn emit_legacy_rule_warnings(config: &Config) {
    let legacy_count = config
        .rules
        .iter()
        .filter(|r| !r.category.is_empty() && !r.description.is_empty())
        .count();

    if legacy_count > 0 {
        eprintln!("warning: adr-fmt.toml contains {legacy_count} legacy rule declaration(s)");
        eprintln!("         Rules are now hardcoded in the binary. Only parameter overrides");
        eprintln!("         are needed in config. Remove `category` and `description` fields.");
        eprintln!("         Example override: [[rules]]");
        eprintln!("         id = \"T015\"");
        eprintln!("         params = {{ min_words = 7, max_words = 100 }}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal_config_no_rules() {
        let toml_str = r#"
[corpus]
root = "docs/adr"

[stale]
directory = "stale"

[[domains]]
prefix = "CHE"
name = "Cherry"
directory = "cherry"
description = "Test domain"
crates = ["example-core"]
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.stale.directory, "stale");
        assert_eq!(config.domains.len(), 1);
        assert_eq!(config.domains[0].prefix, "CHE");
        assert_eq!(config.domains[0].crates, vec!["example-core"]);
        assert!(config.rules.is_empty());
    }

    #[test]
    fn parse_config_with_overrides() {
        let toml_str = r#"
[corpus]
root = "docs/adr"

[stale]
directory = "stale"

[[domains]]
prefix = "CHE"
name = "Cherry"
directory = "cherry"
description = "Test"
crates = []

[[rules]]
id = "T015"
params = { min_words = 7, max_words = 50 }
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.rules.len(), 1);
        assert_eq!(config.rules[0].id, "T015");
        assert_eq!(config.rule_param_u64("T015", "min_words"), Some(7));
        assert_eq!(config.rule_param_u64("T015", "max_words"), Some(50));
    }

    #[test]
    fn parse_multi_domain_config() {
        let toml_str = r#"
[corpus]
root = "docs/adr"

[stale]
directory = "stale"

[[domains]]
prefix = "COM"
name = "Common"
directory = "common"
description = "Cross-cutting"
crates = []

[[domains]]
prefix = "CHE"
name = "Cherry"
directory = "cherry"
description = "Architecture"
crates = ["example-core", "example-gateway"]
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.domains.len(), 2);
        assert_eq!(config.domains[0].prefix, "COM");
        assert!(config.domains[0].crates.is_empty());
        assert_eq!(config.domains[1].crates.len(), 2);
    }

    #[test]
    fn parse_rule_with_params() {
        let toml_str = r#"
[corpus]
root = "docs/adr"

[stale]
directory = "stale"

[[domains]]
prefix = "CHE"
name = "Cherry"
directory = "cherry"
description = "Test"
crates = []

[[rules]]
id = "T015"
params = { min_words = 10 }
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.rules[0].id, "T015");
        let min_words = config.rule_param_u64("T015", "min_words");
        assert_eq!(min_words, Some(10));
    }

    #[test]
    fn rule_param_missing_returns_none() {
        let toml_str = r#"
[corpus]
root = "docs/adr"

[stale]
directory = "stale"

[[domains]]
prefix = "CHE"
name = "Cherry"
directory = "cherry"
description = "Test"
crates = []
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.rule_param_u64("T020", "min_words"), None);
        assert_eq!(config.rule_param_u64("MISSING", "key"), None);
    }

    #[test]
    fn missing_required_field_fails() {
        let toml_str = r#"
[corpus]
root = "docs/adr"

[stale]
directory = "stale"

[[domains]]
prefix = "CHE"
name = "Cherry"
# missing directory and description
"#;
        let result: Result<Config, _> = toml::from_str(toml_str);
        assert!(result.is_err());
    }

    #[test]
    fn foundation_flag_defaults_to_false() {
        let toml_str = r#"
[corpus]
root = "docs/adr"

[stale]
directory = "stale"

[[domains]]
prefix = "CHE"
name = "Cherry"
directory = "cherry"
description = "Test"
crates = []
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert!(!config.domains[0].foundation);
    }

    #[test]
    fn foundation_flag_true_deserializes() {
        let toml_str = r#"
[corpus]
root = "docs/adr"

[stale]
directory = "stale"

[[domains]]
prefix = "COM"
name = "Common"
directory = "common"
description = "Cross-cutting"
crates = []
foundation = true
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert!(config.domains[0].foundation);
    }

    #[test]
    fn legacy_format_still_parses() {
        let toml_str = r#"
[corpus]
root = "docs/adr"

[stale]
directory = "stale"

[[domains]]
prefix = "CHE"
name = "Cherry"
directory = "cherry"
description = "Test"
crates = []

[[rules]]
id = "T020"
category = "template"
description = "Reference load"

[[rules]]
id = "T002"
category = "template"
description = "Date field present"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.rules.len(), 2);
    }

    #[test]
    fn missing_corpus_table_emits_clear_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let toml_str = r#"
[stale]
directory = "stale"

[[domains]]
prefix = "CHE"
name = "Cherry"
directory = "cherry"
description = "Test"
crates = []
"#;
        std::fs::write(dir.path().join("adr-fmt.toml"), toml_str).unwrap();
        let err = match load_quiet(dir.path()).unwrap_err() {
            LoadError::Parse(m) => m,
            LoadError::Io(m) => panic!("expected Parse error, got Io: {m}"),
        };
        assert!(
            err.contains("`[corpus]`"),
            "error must name the [corpus] table; got: {err}"
        );
        assert!(
            err.contains("root = \"docs/adr\""),
            "error must show example; got: {err}"
        );
    }

    #[test]
    fn resolve_corpus_root_returns_canonical_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("docs/adr")).unwrap();
        let corpus = CorpusConfig {
            root: "docs/adr".to_owned(),
        };
        let resolved = resolve_corpus_root(dir.path(), &corpus).expect("resolves");
        assert!(resolved.ends_with("docs/adr"));
    }

    #[test]
    fn resolve_corpus_root_rejects_absolute() {
        let dir = tempfile::tempdir().expect("tempdir");
        let corpus = CorpusConfig {
            root: "/etc".to_owned(),
        };
        let err = resolve_corpus_root(dir.path(), &corpus).unwrap_err();
        assert!(err.contains("absolute"), "got: {err}");
    }

    #[test]
    fn resolve_corpus_root_rejects_parent_traversal() {
        let dir = tempfile::tempdir().expect("tempdir");
        let corpus = CorpusConfig {
            root: "../escape".to_owned(),
        };
        let err = resolve_corpus_root(dir.path(), &corpus).unwrap_err();
        assert!(err.contains("parent-traversal"), "got: {err}");
    }
}
