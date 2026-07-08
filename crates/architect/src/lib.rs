#![forbid(unsafe_code)]

mod adr;
mod fence;
mod manifest;
mod render;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

pub struct Config {
    workspace_root: PathBuf,
    out_dir: PathBuf,
    subfolder: PathBuf,
}

impl Config {
    #[must_use]
    pub fn new(workspace_root: PathBuf, out_dir: PathBuf, subfolder: Option<PathBuf>) -> Self {
        let subfolder = subfolder
            .unwrap_or_else(|| PathBuf::from("Code").join(repo_display_name(&workspace_root)));
        Self {
            workspace_root,
            out_dir,
            subfolder,
        }
    }
}

pub struct Report {
    pub written: Vec<PathBuf>,
}

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error("refused: output subfolder resolves to the --out root; pass a non-empty --subfolder")]
    SubfolderIsRoot,
    #[error(
        "refused: --subfolder must stay under --out; parent-directory segments are not allowed"
    )]
    SubfolderEscapesOut,
    #[error("refused: target path matches the protected hand-authored note {0}")]
    ProtectedPath(PathBuf),
    #[error("failed to read manifest {path}: {source}")]
    ReadManifest {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to parse manifest {path}: {source}")]
    ParseManifest {
        path: PathBuf,
        source: toml::de::Error,
    },
    #[error("workspace manifest {0} has no [workspace.members] array")]
    NoMembers(PathBuf),
    #[error("failed to create output directory {path}: {source}")]
    CreateOutDir {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to read existing note {path}: {source}")]
    ReadNote {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to write note {path}: {source}")]
    WriteNote {
        path: PathBuf,
        source: std::io::Error,
    },
}

const PROTECTED_RELATIVE_NOTE: &str = "Projects/gh-report.md";

/// Scans `config`'s workspace `[workspace.members]` and writes one fenced
/// note per member (excluding this generator's own package), plus an
/// overview note and a decisions-index note linking ADR ids resolved via
/// `adr-fmt --context <crate>`. Content outside the
/// `%% architect:begin/end <id> %%` markers is preserved byte-for-byte on
/// re-run; a crate missing a README or governing ADRs gets a graceful
/// placeholder section rather than failing the run.
///
/// # Errors
///
/// Returns `Err` when the resolved target is the `--out` root, escapes
/// `--out` via a parent-directory segment, or matches the protected
/// hand-authored note path; when the workspace manifest or a member
/// manifest cannot be read or parsed, or declares no
/// `[workspace.members]`; when the output directory cannot be created; or
/// when a note cannot be read back or written.
pub fn run(config: &Config) -> Result<Report, Error> {
    guard_target(&config.out_dir, &config.subfolder)?;
    let target_dir = config.out_dir.join(&config.subfolder);
    std::fs::create_dir_all(&target_dir).map_err(|source| Error::CreateOutDir {
        path: target_dir.clone(),
        source,
    })?;

    let repo_name = repo_display_name(&config.workspace_root);
    let members = manifest::member_crates(&config.workspace_root, env!("CARGO_PKG_NAME"))?;

    let mut docs = Vec::with_capacity(members.len());
    let mut written = Vec::with_capacity(members.len() + 2);
    let mut adr_index: BTreeMap<String, (String, BTreeSet<String>)> = BTreeMap::new();

    for member in &members {
        let doc = manifest::gather(member)?;
        let adrs = adr::governing_adrs(&doc.name);
        for reference in &adrs {
            adr_index
                .entry(reference.id.clone())
                .or_insert_with(|| (reference.title.clone(), BTreeSet::new()))
                .1
                .insert(doc.name.clone());
        }
        let body = render::crate_note_body(&doc, &adrs);
        let path = target_dir.join(format!("{}.md", doc.name));
        write_guarded_note(&config.out_dir, &path, &doc.name, &body)?;
        written.push(path);
        docs.push(doc);
    }

    let overview_path = target_dir.join("Overview.md");
    write_guarded_note(
        &config.out_dir,
        &overview_path,
        "overview",
        &render::overview_body(&repo_name, &docs),
    )?;
    written.push(overview_path);

    let decisions_path = target_dir.join("Decisions.md");
    write_guarded_note(
        &config.out_dir,
        &decisions_path,
        "decisions-index",
        &render::decisions_index_body(&adr_index),
    )?;
    written.push(decisions_path);

    Ok(Report { written })
}

fn write_guarded_note(
    out_dir: &Path,
    path: &Path,
    section_id: &str,
    body: &str,
) -> Result<(), Error> {
    let protected = out_dir.join(PROTECTED_RELATIVE_NOTE);
    if path == protected {
        return Err(Error::ProtectedPath(protected));
    }
    fence::write_note(path, section_id, body)
}

fn guard_target(out_dir: &Path, subfolder: &Path) -> Result<(), Error> {
    if subfolder.is_absolute() {
        return Err(Error::SubfolderEscapesOut);
    }
    if subfolder.as_os_str().is_empty() || subfolder == Path::new(".") {
        return Err(Error::SubfolderIsRoot);
    }
    if subfolder
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(Error::SubfolderEscapesOut);
    }
    if out_dir.join(subfolder) == *out_dir {
        return Err(Error::SubfolderIsRoot);
    }
    Ok(())
}

fn repo_display_name(workspace_root: &Path) -> String {
    if let Some(name) = workspace_root.file_name() {
        return name.to_string_lossy().into_owned();
    }
    if let Some(name) = workspace_root
        .canonicalize()
        .ok()
        .as_deref()
        .and_then(Path::file_name)
    {
        return name.to_string_lossy().into_owned();
    }
    "workspace".to_string()
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{Error, guard_target, repo_display_name};

    #[test]
    fn guard_target_refuses_empty_subfolder() {
        let out = PathBuf::from("/tmp/out");
        let err = guard_target(&out, Path::new("")).unwrap_err();
        assert!(matches!(err, Error::SubfolderIsRoot));
    }

    #[test]
    fn guard_target_refuses_dot_subfolder() {
        let out = PathBuf::from("/tmp/out");
        let err = guard_target(&out, Path::new(".")).unwrap_err();
        assert!(matches!(err, Error::SubfolderIsRoot));
    }

    #[test]
    fn guard_target_refuses_parent_escape() {
        let out = PathBuf::from("/tmp/out");
        let err = guard_target(&out, Path::new("../elsewhere")).unwrap_err();
        assert!(matches!(err, Error::SubfolderEscapesOut));
    }

    #[test]
    fn guard_target_refuses_absolute_subfolder() {
        let out = PathBuf::from("/tmp/out");
        let err = guard_target(&out, Path::new("/etc/whatever")).unwrap_err();
        assert!(matches!(err, Error::SubfolderEscapesOut));
    }

    #[test]
    fn guard_target_accepts_dedicated_subfolder() {
        let out = PathBuf::from("/tmp/out");
        assert!(guard_target(&out, Path::new("Code/gh-report")).is_ok());
    }

    #[test]
    fn repo_display_name_uses_final_path_component() {
        assert_eq!(repo_display_name(Path::new("/a/b/gh-report")), "gh-report");
    }

    #[test]
    fn repo_display_name_canonicalizes_a_bare_dot() {
        let name = repo_display_name(Path::new("."));
        assert_ne!(name, "workspace");
    }
}
