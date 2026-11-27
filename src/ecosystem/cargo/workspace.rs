//! Workspace discovery from manifests alone: members, path dependencies,
//! declared dependencies and unsupported inputs. No Cargo process is run.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use toml::{Table, Value};

use crate::model::UnsupportedInput;
use crate::{Error, Result};

const DEPENDENCY_TABLES: [(&str, &str); 3] = [
    ("dependencies", "normal"),
    ("dev-dependencies", "dev"),
    ("build-dependencies", "build"),
];

/// One package manifest inside the workspace root.
#[derive(Debug, Clone)]
pub(crate) struct PackageManifest {
    /// Directory of the manifest relative to the workspace root; `.` for the
    /// root itself. Always uses `/`.
    pub rel_dir: String,
    pub name: String,
    pub version: String,
    /// Whether Cargo treats it as a workspace member.
    pub is_member: bool,
    pub table: Table,
}

/// What the manifests of a workspace say, without asking Cargo.
#[derive(Debug, Clone)]
pub(crate) struct Workspace {
    pub root: PathBuf,
    pub root_table: Table,
    /// Every package manifest reachable from the root: members and path
    /// dependencies. Sorted by `rel_dir`.
    pub packages: Vec<PackageManifest>,
    pub unsupported: Vec<UnsupportedInput>,
}

fn read_table(path: &Path) -> Result<Table> {
    let text = fs::read_to_string(path).map_err(|e| Error::io("read", path, e))?;
    text.parse::<Table>()
        .map_err(|e| Error::parse("manifest", path, e.to_string()))
}

/// Joins `relative` onto `base` and resolves `.` and `..` lexically. Returns
/// `None` when the result would leave the root that `base` is relative to.
fn join_inside(base: &str, relative: &str) -> Option<String> {
    let mut parts: Vec<String> = if base == "." {
        Vec::new()
    } else {
        base.split('/').map(str::to_string).collect()
    };
    for component in Path::new(relative).components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
            Component::ParentDir => {
                parts.pop()?;
            }
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(if parts.is_empty() {
        ".".to_string()
    } else {
        parts.join("/")
    })
}

fn rel_path(root: &Path, rel_dir: &str) -> PathBuf {
    if rel_dir == "." {
        root.to_path_buf()
    } else {
        root.join(rel_dir)
    }
}

fn manifest_label(rel_dir: &str) -> String {
    if rel_dir == "." {
        "Cargo.toml".to_string()
    } else {
        format!("{rel_dir}/Cargo.toml")
    }
}

/// A `*` and `?` wildcard match within one path segment.
fn segment_matches(pattern: &str, name: &str) -> bool {
    fn go(p: &[char], n: &[char]) -> bool {
        match (p.first(), n.first()) {
            (None, None) => true,
            (Some('*'), _) => go(&p[1..], n) || (!n.is_empty() && go(p, &n[1..])),
            (Some('?'), Some(_)) => go(&p[1..], &n[1..]),
            (Some(a), Some(b)) if a == b => go(&p[1..], &n[1..]),
            _ => false,
        }
    }
    go(
        &pattern.chars().collect::<Vec<_>>(),
        &name.chars().collect::<Vec<_>>(),
    )
}

/// Expands one `members` entry to relative directories that exist.
fn expand_member(root: &Path, pattern: &str) -> Result<Vec<String>> {
    let mut found = vec![".".to_string()];
    for segment in pattern.split('/').filter(|s| !s.is_empty() && *s != ".") {
        let mut next = Vec::new();
        for base in &found {
            if segment.contains('*') || segment.contains('?') {
                let dir = rel_path(root, base);
                let entries = match fs::read_dir(&dir) {
                    Ok(entries) => entries,
                    Err(_) => continue,
                };
                for entry in entries {
                    let entry = entry.map_err(|e| Error::io("read", &dir, e))?;
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if entry.path().is_dir() && segment_matches(segment, &name) {
                        if let Some(joined) = join_inside(base, &name) {
                            next.push(joined);
                        }
                    }
                }
            } else if let Some(joined) = join_inside(base, segment) {
                next.push(joined);
            }
        }
        found = next;
    }
    found.sort();
    Ok(found)
}

fn strings(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Every dependency table of a manifest as `(kind, target, table)`.
fn dependency_tables(table: &Table) -> Vec<(&'static str, String, &Table)> {
    let mut tables = Vec::new();
    for (key, kind) in DEPENDENCY_TABLES {
        if let Some(deps) = table.get(key).and_then(Value::as_table) {
            tables.push((kind, "-".to_string(), deps));
        }
    }
    if let Some(targets) = table.get("target").and_then(Value::as_table) {
        for (target, value) in targets {
            for (key, kind) in DEPENDENCY_TABLES {
                if let Some(deps) = value.get(key).and_then(Value::as_table) {
                    tables.push((kind, target.clone(), deps));
                }
            }
        }
    }
    tables
}

impl Workspace {
    /// Reads the workspace rooted at `root`.
    pub fn discover(root: &Path) -> Result<Workspace> {
        let root = root
            .canonicalize()
            .map_err(|e| Error::io("open", root, e))?;
        let root_manifest = root.join("Cargo.toml");
        if !root_manifest.is_file() {
            return Err(Error::parse(
                "manifest",
                &root_manifest,
                "no Cargo.toml here; point depvigil at the workspace root",
            ));
        }
        let root_table = read_table(&root_manifest)?;
        let workspace_table = root_table.get("workspace").and_then(Value::as_table);
        let excluded: Vec<String> = strings(workspace_table.and_then(|w| w.get("exclude")))
            .iter()
            .filter_map(|e| join_inside(".", e))
            .collect();
        let is_excluded = |rel_dir: &str| {
            excluded
                .iter()
                .any(|e| rel_dir == e || rel_dir.starts_with(&format!("{e}/")))
        };

        let mut unsupported = BTreeSet::new();
        for section in ["patch", "replace"] {
            if root_table.contains_key(section) {
                unsupported.insert(UnsupportedInput::PatchSection {
                    file: "Cargo.toml".into(),
                });
            }
        }
        let cargo_config = root.join(".cargo").join("config.toml");
        if cargo_config.is_file() && read_table(&cargo_config)?.contains_key("patch") {
            unsupported.insert(UnsupportedInput::PatchSection {
                file: ".cargo/config.toml".into(),
            });
        }

        // Explicit members, then the root package.
        let mut queue: Vec<(String, bool)> = Vec::new();
        if root_table.contains_key("package") {
            queue.push((".".to_string(), true));
        }
        for pattern in strings(workspace_table.and_then(|w| w.get("members"))) {
            for rel_dir in expand_member(&root, &pattern)? {
                if !is_excluded(&rel_dir) && rel_path(&root, &rel_dir).join("Cargo.toml").is_file()
                {
                    queue.push((rel_dir, true));
                }
            }
        }

        // Path dependencies declared at workspace level are relative to the root.
        let mut workspace_paths: BTreeMap<String, String> = BTreeMap::new();
        if let Some(deps) = workspace_table
            .and_then(|w| w.get("dependencies"))
            .and_then(Value::as_table)
        {
            for (name, value) in deps {
                if value.get("git").is_some() {
                    unsupported.insert(UnsupportedInput::GitDependency {
                        manifest: "Cargo.toml".into(),
                        name: name.clone(),
                    });
                }
                if let Some(path) = value.get("path").and_then(Value::as_str) {
                    match join_inside(".", path) {
                        Some(rel_dir) => {
                            workspace_paths.insert(name.clone(), rel_dir);
                        }
                        None => {
                            unsupported.insert(UnsupportedInput::PathOutsideRoot {
                                manifest: "Cargo.toml".into(),
                                name: name.clone(),
                                path: path.to_string(),
                            });
                        }
                    }
                }
            }
        }

        let has_workspace = workspace_table.is_some();
        let mut packages: BTreeMap<String, PackageManifest> = BTreeMap::new();
        while let Some((rel_dir, is_member)) = queue.pop() {
            if packages.contains_key(&rel_dir) {
                continue;
            }
            let manifest_path = rel_path(&root, &rel_dir).join("Cargo.toml");
            let table = read_table(&manifest_path)?;
            let Some(package) = table.get("package").and_then(Value::as_table) else {
                continue; // a virtual manifest has no package of its own
            };
            let name = package
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| Error::parse("manifest", &manifest_path, "package has no name"))?
                .to_string();
            let version = match package.get("version") {
                Some(Value::String(version)) => version.clone(),
                Some(Value::Table(_)) => workspace_table
                    .and_then(|w| w.get("package"))
                    .and_then(|p| p.get("version"))
                    .and_then(Value::as_str)
                    .unwrap_or("0.0.0")
                    .to_string(),
                _ => "0.0.0".to_string(),
            };

            for (_, _, deps) in dependency_tables(&table) {
                for (dep_name, value) in deps {
                    if value.get("git").is_some() {
                        unsupported.insert(UnsupportedInput::GitDependency {
                            manifest: manifest_label(&rel_dir),
                            name: dep_name.clone(),
                        });
                    }
                    let inherited = value.get("workspace").and_then(Value::as_bool) == Some(true);
                    let target_dir = if let Some(path) = value.get("path").and_then(Value::as_str) {
                        match join_inside(&rel_dir, path) {
                            Some(dir) => Some(dir),
                            None => {
                                unsupported.insert(UnsupportedInput::PathOutsideRoot {
                                    manifest: manifest_label(&rel_dir),
                                    name: dep_name.clone(),
                                    path: path.to_string(),
                                });
                                None
                            }
                        }
                    } else if inherited {
                        workspace_paths.get(dep_name).cloned()
                    } else {
                        None
                    };
                    if let Some(dir) = target_dir {
                        // A path dependency inside the workspace directory is
                        // a member unless the workspace excludes it.
                        let member = has_workspace && !is_excluded(&dir);
                        queue.push((dir, member));
                    }
                }
            }
            packages.insert(
                rel_dir.clone(),
                PackageManifest {
                    rel_dir,
                    name,
                    version,
                    is_member,
                    table,
                },
            );
        }

        // A package can be reached both as a member and as a path dependency.
        Ok(Workspace {
            root,
            root_table,
            packages: packages.into_values().collect(),
            unsupported: unsupported.into_iter().collect(),
        })
    }

    /// The relative directory of the path package with this name and version.
    pub fn path_package(&self, name: &str, version: &str) -> Option<&PackageManifest> {
        self.packages
            .iter()
            .find(|p| p.name == name && p.version == version)
            .or_else(|| self.packages.iter().find(|p| p.name == name))
    }

    /// Workspace members, sorted by directory.
    pub fn members(&self) -> impl Iterator<Item = &PackageManifest> {
        self.packages.iter().filter(|p| p.is_member)
    }

    /// A short, path-free name for the project.
    pub fn project_name(&self) -> String {
        self.root_table
            .get("package")
            .and_then(|p| p.get("name"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                self.root
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| "workspace".to_string())
    }

    /// Canonical lines for everything the workspace declares (ADR 0003):
    /// the resolver, and each member's dependencies and features.
    pub fn declared_lines(&self) -> Vec<String> {
        let workspace_table = self.root_table.get("workspace").and_then(Value::as_table);
        let resolver = workspace_table
            .and_then(|w| w.get("resolver"))
            .or_else(|| {
                self.root_table
                    .get("package")
                    .and_then(|p| p.get("resolver"))
            })
            .and_then(Value::as_str)
            .unwrap_or("unset");
        let workspace_deps = workspace_table
            .and_then(|w| w.get("dependencies"))
            .and_then(Value::as_table);

        let mut lines = vec![format!("resolver {resolver}")];
        for member in self.members() {
            for (kind, target, deps) in dependency_tables(&member.table) {
                for (key, value) in deps {
                    lines.push(format!(
                        "declared {} {kind} {target} {}",
                        member.rel_dir,
                        declared_dependency(key, value, &member.rel_dir, workspace_deps)
                    ));
                }
            }
            if let Some(features) = member.table.get("features").and_then(Value::as_table) {
                for (feature, enables) in features {
                    let mut enables = strings(Some(enables));
                    enables.sort();
                    lines.push(format!(
                        "feature {} {feature} [{}]",
                        member.rel_dir,
                        enables.join(",")
                    ));
                }
            }
        }
        lines.sort();
        lines
    }
}

/// One declared dependency in canonical form: the key, the real package
/// name, the requirement, where it comes from, and its feature settings.
fn declared_dependency(
    key: &str,
    value: &Value,
    rel_dir: &str,
    workspace_deps: Option<&Table>,
) -> String {
    let inherited = value.get("workspace").and_then(Value::as_bool) == Some(true);
    let base = if inherited {
        workspace_deps.and_then(|w| w.get(key))
    } else {
        None
    };
    // A member's own setting wins over the inherited one.
    let field = |name: &str| value.get(name).or_else(|| base.and_then(|b| b.get(name)));
    let requirement = match (value, base) {
        (Value::String(req), _) => req.clone(),
        (_, Some(Value::String(req))) => req.clone(),
        _ => field("version")
            .and_then(Value::as_str)
            .unwrap_or("*")
            .to_string(),
    };
    let package = field("package").and_then(Value::as_str).unwrap_or(key);
    let source = if field("git").is_some() {
        "git".to_string()
    } else if let Some(path) = value.get("path").and_then(Value::as_str) {
        format!(
            "path:{}",
            join_inside(rel_dir, path).unwrap_or_else(|| "outside".into())
        )
    } else if let Some(path) = base.and_then(|b| b.get("path")).and_then(Value::as_str) {
        format!(
            "path:{}",
            join_inside(".", path).unwrap_or_else(|| "outside".into())
        )
    } else {
        field("registry")
            .and_then(Value::as_str)
            .unwrap_or("default")
            .to_string()
    };
    // Features of an inherited dependency add to the workspace's.
    let mut features: BTreeSet<String> = strings(value.get("features")).into_iter().collect();
    features.extend(strings(base.and_then(|b| b.get("features"))));
    let default_features = field("default-features")
        .or_else(|| field("default_features"))
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let optional = value
        .get("optional")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    format!(
        "{key} package={package} req={requirement} source={source} features=[{}] default-features={default_features} optional={optional}",
        features.into_iter().collect::<Vec<_>>().join(",")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_inside_resolves_and_refuses_escape() {
        assert_eq!(join_inside("app", "../core").as_deref(), Some("core"));
        assert_eq!(
            join_inside(".", "vendor/marker").as_deref(),
            Some("vendor/marker")
        );
        assert_eq!(join_inside("app", "..").as_deref(), Some("."));
        assert_eq!(join_inside("app", "../../elsewhere"), None);
        assert_eq!(join_inside(".", "/abs"), None);
    }

    #[test]
    fn segment_wildcards() {
        assert!(segment_matches("*", "anything"));
        assert!(segment_matches("crate-*", "crate-a"));
        assert!(!segment_matches("crate-*", "other"));
        assert!(segment_matches("a?c", "abc"));
    }

    #[test]
    fn declared_dependency_canonical_form() {
        let table: Table = "dep = { version = \"1\", features = [\"b\", \"a\"], default-features = false, optional = true, package = \"real\" }\nplain = \"0.4\""
            .parse()
            .unwrap();
        assert_eq!(
            declared_dependency("dep", &table["dep"], "app", None),
            "dep package=real req=1 source=default features=[a,b] default-features=false optional=true"
        );
        assert_eq!(
            declared_dependency("plain", &table["plain"], "app", None),
            "plain package=plain req=0.4 source=default features=[] default-features=true optional=false"
        );
    }
}
