use std::path::{Path, PathBuf};

use toml::Value;

use crate::Error;

pub(crate) struct WorkspaceCrate {
    name: String,
    relative_dir: String,
    dir: PathBuf,
}

pub(crate) struct CrateDoc {
    pub(crate) name: String,
    pub(crate) description: Option<String>,
    pub(crate) dependencies: Vec<String>,
    pub(crate) readme_relative_path: Option<String>,
}

pub(crate) fn member_crates(
    workspace_root: &Path,
    exclude_package: &str,
) -> Result<Vec<WorkspaceCrate>, Error> {
    let manifest_path = workspace_root.join("Cargo.toml");
    let root_value = read_toml(&manifest_path)?;

    let members = root_value
        .get("workspace")
        .and_then(|workspace| workspace.get("members"))
        .and_then(Value::as_array)
        .ok_or_else(|| Error::NoMembers(manifest_path.clone()))?;

    let mut crates = Vec::with_capacity(members.len());
    for member in members {
        let Some(relative_dir) = member.as_str() else {
            continue;
        };
        let dir = workspace_root.join(relative_dir);
        let member_value = read_toml(&dir.join("Cargo.toml"))?;
        let name = member_value
            .get("package")
            .and_then(|package| package.get("name"))
            .and_then(Value::as_str)
            .map_or_else(|| fallback_name(relative_dir), ToString::to_string);
        if name == exclude_package {
            continue;
        }
        crates.push(WorkspaceCrate {
            name,
            relative_dir: relative_dir.to_string(),
            dir,
        });
    }
    crates.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(crates)
}

fn fallback_name(relative_dir: &str) -> String {
    relative_dir
        .rsplit('/')
        .next()
        .unwrap_or(relative_dir)
        .to_string()
}

pub(crate) fn gather(member: &WorkspaceCrate) -> Result<CrateDoc, Error> {
    let value = read_toml(&member.dir.join("Cargo.toml"))?;

    let description = value
        .get("package")
        .and_then(|package| package.get("description"))
        .and_then(Value::as_str)
        .map(ToString::to_string);

    let mut dependencies: Vec<String> = value
        .get("dependencies")
        .and_then(Value::as_table)
        .map(|table| table.keys().cloned().collect())
        .unwrap_or_default();
    dependencies.sort();

    let readme_relative_path = member.dir.join("README.md").is_file().then(|| {
        let relative_dir = &member.relative_dir;
        format!("{relative_dir}/README.md")
    });

    Ok(CrateDoc {
        name: member.name.clone(),
        description,
        dependencies,
        readme_relative_path,
    })
}

fn read_toml(path: &Path) -> Result<Value, Error> {
    let text = std::fs::read_to_string(path).map_err(|source| Error::ReadManifest {
        path: path.to_path_buf(),
        source,
    })?;
    toml::from_str(&text).map_err(|source| Error::ParseManifest {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{gather, member_crates};

    #[test]
    fn member_crates_excludes_self_and_reads_names_from_manifests() {
        let root = tempfile::tempdir().expect("tempdir");
        fs::write(
            root.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/alpha\", \"crates/architect\"]\n",
        )
        .expect("write root manifest");
        fs::create_dir_all(root.path().join("crates/alpha")).expect("mkdir alpha");
        fs::write(
            root.path().join("crates/alpha/Cargo.toml"),
            "[package]\nname = \"alpha\"\ndescription = \"Alpha crate\"\n\n[dependencies]\nserde = \"1\"\n",
        )
        .expect("write alpha manifest");
        fs::create_dir_all(root.path().join("crates/architect")).expect("mkdir architect");
        fs::write(
            root.path().join("crates/architect/Cargo.toml"),
            "[package]\nname = \"architect\"\n",
        )
        .expect("write architect manifest");

        let crates = member_crates(root.path(), "architect").expect("member_crates");

        assert_eq!(crates.len(), 1);
        assert_eq!(crates[0].name, "alpha");

        let doc = gather(&crates[0]).expect("gather");
        assert_eq!(doc.description.as_deref(), Some("Alpha crate"));
        assert_eq!(doc.dependencies, vec!["serde".to_string()]);
        assert!(doc.readme_relative_path.is_none());
    }

    #[test]
    fn gather_reports_readme_when_present() {
        let root = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(root.path().join("crates/beta")).expect("mkdir beta");
        fs::write(
            root.path().join("crates/beta/Cargo.toml"),
            "[package]\nname = \"beta\"\n",
        )
        .expect("write beta manifest");
        fs::write(root.path().join("crates/beta/README.md"), "# beta\n").expect("write readme");
        fs::write(
            root.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/beta\"]\n",
        )
        .expect("write root manifest");

        let crates = member_crates(root.path(), "architect").expect("member_crates");
        let doc = gather(&crates[0]).expect("gather");

        assert_eq!(
            doc.readme_relative_path.as_deref(),
            Some("crates/beta/README.md")
        );
        assert!(doc.description.is_none());
        assert!(doc.dependencies.is_empty());
    }
}
