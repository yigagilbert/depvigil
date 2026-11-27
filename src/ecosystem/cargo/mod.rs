//! The Cargo implementation of [`Ecosystem`].
//!
//! Safety limits, enforced here:
//! - only `cargo add`, `cargo update`, `cargo metadata` and `cargo tree` can
//!   run, and only inside a temporary manifest-only copy;
//! - nothing is ever built, so no third-party code and no build script runs;
//! - the project directory is only ever read.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use toml::{Table, Value};

use crate::ecosystem::{Candidate, Ecosystem};
use crate::model::{
    DependencyEdge, EdgeKind, PackageId, Precision, ProposedChange, ResolvedPackage, ResolvedState,
    Source, UnsupportedInput,
};
use crate::{Error, Result};

mod workspace;

use workspace::Workspace;

const ECOSYSTEM: &str = "cargo";

/// Cargo subcommands that only resolve. Nothing else may run on a candidate.
const ALLOWED_SUBCOMMANDS: [&str; 4] = ["add", "update", "metadata", "tree"];

/// Variables Cargo sets for the processes it spawns. They describe the
/// package being built, not the workspace being simulated, so they must not
/// leak into a simulation started from inside `cargo test` or `cargo run`.
const CARGO_BUILD_ENV_PREFIXES: [&str; 5] = [
    "CARGO_MANIFEST_",
    "CARGO_PKG_",
    "CARGO_BIN_",
    "CARGO_CRATE_",
    "CARGO_PRIMARY_",
];

/// Files other than manifests that affect resolution and are copied along.
const RESOLUTION_FILES: [&str; 4] = [
    "Cargo.lock",
    ".cargo/config.toml",
    "rust-toolchain.toml",
    "rust-toolchain",
];

/// Cargo support.
#[derive(Debug, Default)]
pub struct CargoEcosystem {
    cargo_home: Option<PathBuf>,
    offline: bool,
    invocations: Mutex<Vec<String>>,
}

impl CargoEcosystem {
    /// Cargo with the user's own configuration.
    pub fn new() -> Self {
        Self::default()
    }

    /// Uses this directory as `CARGO_HOME` for every Cargo run.
    pub fn with_cargo_home(mut self, cargo_home: impl Into<PathBuf>) -> Self {
        self.cargo_home = Some(cargo_home.into());
        self
    }

    /// Passes `--offline` to every Cargo run.
    pub fn offline(mut self, offline: bool) -> Self {
        self.offline = offline;
        self
    }

    /// Runs an allowed Cargo subcommand in `dir` and returns its stdout.
    fn cargo(&self, dir: &Path, args: &[String]) -> Result<String> {
        let mut args = args.to_vec();
        if self.offline {
            args.push("--offline".to_string());
        }
        let command_line = format!("cargo {}", args.join(" "));
        let allowed = args
            .first()
            .is_some_and(|sub| ALLOWED_SUBCOMMANDS.contains(&sub.as_str()));
        if !allowed {
            return Err(Error::ToolNotAllowed(command_line));
        }
        let mut command = Command::new("cargo");
        command.current_dir(dir).args(&args);
        for (key, _) in std::env::vars_os() {
            let key_text = key.to_string_lossy();
            if CARGO_BUILD_ENV_PREFIXES
                .iter()
                .any(|prefix| key_text.starts_with(prefix))
            {
                command.env_remove(&key);
            }
        }
        if let Some(home) = &self.cargo_home {
            command.env("CARGO_HOME", home);
        }
        command.env("CARGO_TERM_COLOR", "never");
        if let Ok(mut log) = self.invocations.lock() {
            log.push(command_line.clone());
        }
        let output = command.output().map_err(|source| Error::ToolUnavailable {
            command: command_line.clone(),
            source,
        })?;
        if !output.status.success() {
            return Err(Error::Tool {
                command: command_line,
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
            });
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    fn supported_workspace(&self, project: &Path) -> Result<Workspace> {
        let workspace = Workspace::discover(project)?;
        let mut unsupported = workspace.unsupported.clone();
        let lock_path = workspace.root.join("Cargo.lock");
        if lock_path.is_file() {
            for package in read_lock(&lock_path)? {
                if package
                    .source
                    .as_deref()
                    .is_some_and(|s| s.starts_with("git+"))
                {
                    let input = UnsupportedInput::GitDependency {
                        manifest: "Cargo.lock".to_string(),
                        name: package.name,
                    };
                    if !unsupported.contains(&input) {
                        unsupported.push(input);
                    }
                }
            }
        }
        if unsupported.is_empty() {
            Ok(workspace)
        } else {
            Err(Error::Unsupported(unsupported))
        }
    }

    /// Runs `cargo metadata` in an isolated copy and merges the resolve
    /// graph into a lockfile-precision state.
    fn refine_state(&self, dir: &Path, state: &mut ResolvedState) -> Result<()> {
        let stdout = self.cargo(
            dir,
            &[
                "metadata".to_string(),
                "--format-version".to_string(),
                "1".to_string(),
            ],
        )?;
        let root = dir.canonicalize().map_err(|e| Error::io("open", dir, e))?;
        let metadata: serde_json::Value = serde_json::from_str(&stdout)
            .map_err(|e| Error::parse("cargo metadata output", dir, e.to_string()))?;
        let bad = |what: &str| Error::parse("cargo metadata output", dir, what.to_string());

        // Map Cargo's package ids to our identities.
        let mut purls: BTreeMap<&str, String> = BTreeMap::new();
        for package in metadata["packages"]
            .as_array()
            .ok_or_else(|| bad("no packages"))?
        {
            let id = package["id"]
                .as_str()
                .ok_or_else(|| bad("package without id"))?;
            let name = package["name"]
                .as_str()
                .ok_or_else(|| bad("package without name"))?;
            let version = package["version"]
                .as_str()
                .ok_or_else(|| bad("package without version"))?;
            let source = match package["source"].as_str() {
                Some(source) => Source::from_cargo(source),
                None => {
                    let manifest = package["manifest_path"]
                        .as_str()
                        .ok_or_else(|| bad("path package without manifest_path"))?;
                    let dir = Path::new(manifest)
                        .parent()
                        .and_then(|parent| parent.strip_prefix(&root).ok())
                        .ok_or_else(|| bad("path package outside the workspace root"))?;
                    let rel = dir.to_string_lossy().replace('\\', "/");
                    Source::Path(if rel.is_empty() { ".".to_string() } else { rel })
                }
            };
            let package_id = PackageId {
                name: name.to_string(),
                version: version.to_string(),
                source,
            };
            purls.insert(id, package_id.purl(ECOSYSTEM));
        }
        let purl_of = |id: &serde_json::Value| -> Result<String> {
            id.as_str()
                .and_then(|id| purls.get(id).cloned())
                .ok_or_else(|| bad("resolve graph names an unknown package"))
        };

        let mut active = BTreeSet::new();
        let nodes = metadata["resolve"]["nodes"]
            .as_array()
            .ok_or_else(|| bad("no resolve graph"))?;
        for node in nodes {
            let from = purl_of(&node["id"])?;
            active.insert(from.clone());
            for dep in node["deps"]
                .as_array()
                .ok_or_else(|| bad("node without deps"))?
            {
                let to = purl_of(&dep["pkg"])?;
                for kind in dep["dep_kinds"]
                    .as_array()
                    .ok_or_else(|| bad("dep without kinds"))?
                {
                    let kind = match kind["kind"].as_str() {
                        None => EdgeKind::Normal,
                        Some("dev") => EdgeKind::Dev,
                        Some("build") => EdgeKind::Build,
                        Some(_) => EdgeKind::Normal,
                    };
                    state.edges.insert(DependencyEdge {
                        from: from.clone(),
                        to: to.clone(),
                        kind,
                    });
                }
            }
        }
        // The lockfile must contain the whole resolve graph (ADR 0006).
        let known = state.purls();
        if let Some(missing) = active.iter().find(|purl| !known.contains(*purl)) {
            return Err(bad(&format!(
                "resolve graph package {missing} is not in the lockfile"
            )));
        }
        for package in &mut state.packages {
            package.active = Some(active.contains(&package.purl));
        }
        let mut roots = Vec::new();
        for member in metadata["workspace_members"]
            .as_array()
            .ok_or_else(|| bad("no members"))?
        {
            roots.push(purl_of(member)?);
        }
        roots.sort();
        state.roots = roots;
        state.precision = Precision::Full;
        Ok(())
    }
}

/// One `[[package]]` entry of a lockfile.
struct LockPackage {
    name: String,
    version: String,
    source: Option<String>,
    checksum: Option<String>,
    dependencies: Vec<String>,
}

fn read_lock(path: &Path) -> Result<Vec<LockPackage>> {
    let text = fs::read_to_string(path).map_err(|e| Error::io("read", path, e))?;
    let table: Table = text
        .parse()
        .map_err(|e: toml::de::Error| Error::parse("lockfile", path, e.to_string()))?;
    let mut packages = Vec::new();
    for entry in table
        .get("package")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let field = |key: &str| entry.get(key).and_then(Value::as_str).map(str::to_string);
        packages.push(LockPackage {
            name: field("name")
                .ok_or_else(|| Error::parse("lockfile", path, "package without name"))?,
            version: field("version")
                .ok_or_else(|| Error::parse("lockfile", path, "package without version"))?,
            source: field("source"),
            checksum: field("checksum"),
            dependencies: entry
                .get("dependencies")
                .and_then(Value::as_array)
                .map(|deps| {
                    deps.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
        });
    }
    Ok(packages)
}

/// Builds the lockfile-precision state of the workspace at `dir`, whose
/// manifests are described by `workspace`.
fn lockfile_state(dir: &Path, workspace: &Workspace) -> Result<ResolvedState> {
    let lock_path = dir.join("Cargo.lock");
    if !lock_path.is_file() {
        return Err(Error::parse(
            "lockfile",
            &lock_path,
            "no Cargo.lock; depvigil needs a committed lockfile",
        ));
    }
    let lock = read_lock(&lock_path)?;
    let mut packages = Vec::new();
    for entry in &lock {
        let source = match &entry.source {
            Some(source) => Source::from_cargo(source),
            None => {
                let manifest = workspace
                    .path_package(&entry.name, &entry.version)
                    .ok_or_else(|| {
                        Error::parse(
                            "lockfile",
                            &lock_path,
                            format!(
                                "path package `{}` has no manifest in the workspace",
                                entry.name
                            ),
                        )
                    })?;
                Source::Path(manifest.rel_dir.clone())
            }
        };
        let id = PackageId {
            name: entry.name.clone(),
            version: entry.version.clone(),
            source,
        };
        packages.push(ResolvedPackage {
            purl: id.purl(ECOSYSTEM),
            id,
            checksum: entry.checksum.clone(),
            active: None,
        });
    }

    // A lockfile dependency is "name", "name version" or
    // "name version (source)", as specific as needed to be unambiguous.
    let resolve = |spec: &str| -> Option<String> {
        let mut parts = spec.splitn(3, ' ');
        let name = parts.next()?;
        let version = parts.next();
        let source = parts
            .next()
            .map(|s| s.trim_start_matches('(').trim_end_matches(')'));
        packages
            .iter()
            .zip(&lock)
            .find(|(_, entry)| {
                entry.name == name
                    && version.is_none_or(|v| entry.version == v)
                    && source.is_none_or(|s| entry.source.as_deref() == Some(s))
            })
            .map(|(package, _)| package.purl.clone())
    };
    let mut edges = BTreeSet::new();
    for (package, entry) in packages.iter().zip(&lock) {
        for spec in &entry.dependencies {
            let to = resolve(spec).ok_or_else(|| {
                Error::parse(
                    "lockfile",
                    &lock_path,
                    format!("unknown dependency `{spec}`"),
                )
            })?;
            edges.insert(DependencyEdge {
                from: package.purl.clone(),
                to,
                kind: EdgeKind::Lock,
            });
        }
    }

    let mut roots: Vec<String> = workspace
        .members()
        .filter_map(|member| {
            packages
                .iter()
                .find(|p| {
                    p.id.name == member.name && p.id.source == Source::Path(member.rel_dir.clone())
                })
                .map(|p| p.purl.clone())
        })
        .collect();
    roots.sort();
    packages.sort();
    Ok(ResolvedState {
        ecosystem: ECOSYSTEM.to_string(),
        precision: Precision::Lockfile,
        packages,
        edges,
        roots,
        declared: workspace.declared_lines(),
    })
}

fn copy_into(root: &Path, dest: &Path, relative: &str) -> Result<bool> {
    let from = root.join(relative);
    if !from.is_file() {
        return Ok(false);
    }
    let to = dest.join(relative);
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).map_err(|e| Error::io("create", parent, e))?;
    }
    fs::copy(&from, &to).map_err(|e| Error::io("copy", &from, e))?;
    Ok(true)
}

fn write_stub(path: &Path) -> Result<()> {
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| Error::io("create", parent, e))?;
    }
    fs::write(path, "").map_err(|e| Error::io("write", path, e))
}

/// Copies only what resolution needs into a new temporary directory: the
/// manifests Cargo would read, the lockfile and resolution settings. Source
/// files and build scripts are replaced by empty stubs, because Cargo
/// rejects a manifest with no targets. Nothing in the copy is ever built.
fn isolate(workspace: &Workspace) -> Result<tempfile::TempDir> {
    let temp = tempfile::Builder::new()
        .prefix("depvigil-")
        .tempdir()
        .map_err(|e| Error::io("create", std::env::temp_dir(), e))?;
    let dest = temp.path();
    copy_into(&workspace.root, dest, "Cargo.toml")?;
    for file in RESOLUTION_FILES {
        copy_into(&workspace.root, dest, file)?;
    }
    for package in &workspace.packages {
        let dir = if package.rel_dir == "." {
            String::new()
        } else {
            format!("{}/", package.rel_dir)
        };
        copy_into(&workspace.root, dest, &format!("{dir}Cargo.toml"))?;
        let package_dir = dest.join(&dir);

        let lib_path = package
            .table
            .get("lib")
            .and_then(|lib| lib.get("path"))
            .and_then(Value::as_str)
            .unwrap_or("src/lib.rs");
        write_stub(&package_dir.join(lib_path))?;
        for (kind, default_dir) in [
            ("bin", "src/bin"),
            ("example", "examples"),
            ("test", "tests"),
            ("bench", "benches"),
        ] {
            for target in package
                .table
                .get(kind)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                match (
                    target.get("path").and_then(Value::as_str),
                    target.get("name").and_then(Value::as_str),
                ) {
                    (Some(path), _) => write_stub(&package_dir.join(path))?,
                    (None, Some(name)) => {
                        write_stub(&package_dir.join(default_dir).join(format!("{name}.rs")))?
                    }
                    (None, None) => {}
                }
            }
        }
        if let Some(build) = package
            .table
            .get("package")
            .and_then(|p| p.get("build"))
            .and_then(Value::as_str)
        {
            write_stub(&package_dir.join(build))?;
        }
    }
    Ok(temp)
}

fn add_arguments(change: &ProposedChange, workspace: &Workspace) -> Result<Vec<String>> {
    let mut args = vec!["add".to_string(), change.spec()];
    let members: Vec<&str> = workspace.members().map(|m| m.name.as_str()).collect();
    match &change.member {
        Some(member) => {
            if !members.contains(&member.as_str()) {
                return Err(Error::InvalidInput(format!(
                    "`{member}` is not a workspace member; members are: {}",
                    members.join(", ")
                )));
            }
            args.push("--package".to_string());
            args.push(member.clone());
        }
        None if members.len() > 1 => {
            return Err(Error::InvalidInput(format!(
                "this workspace has several members; choose one with --package ({})",
                members.join(", ")
            )));
        }
        None => {}
    }
    if !change.features.is_empty() {
        let valid = change
            .features
            .iter()
            .all(|f| !f.is_empty() && !f.starts_with('-') && !f.contains(char::is_whitespace));
        if !valid {
            return Err(Error::InvalidInput("invalid feature name".to_string()));
        }
        args.push("--features".to_string());
        args.push(change.features.join(","));
    }
    if change.no_default_features {
        args.push("--no-default-features".to_string());
    }
    if change.optional {
        args.push("--optional".to_string());
    }
    Ok(args)
}

impl Ecosystem for CargoEcosystem {
    fn name(&self) -> &'static str {
        ECOSYSTEM
    }

    fn project_name(&self, project: &Path) -> Result<String> {
        Ok(Workspace::discover(project)?.project_name())
    }

    fn unsupported_inputs(&self, project: &Path) -> Result<Vec<UnsupportedInput>> {
        match self.supported_workspace(project) {
            Ok(_) => Ok(Vec::new()),
            Err(Error::Unsupported(inputs)) => Ok(inputs),
            Err(other) => Err(other),
        }
    }

    fn current_state(&self, project: &Path, precision: Precision) -> Result<ResolvedState> {
        let workspace = self.supported_workspace(project)?;
        let mut state = lockfile_state(&workspace.root, &workspace)?;
        if precision == Precision::Full {
            // Resolve in a copy so the project is never written to.
            let copy = isolate(&workspace)?;
            let before = fs::read(copy.path().join("Cargo.lock"))
                .map_err(|e| Error::io("read", copy.path(), e))?;
            self.refine_state(copy.path(), &mut state)?;
            let after = fs::read(copy.path().join("Cargo.lock"))
                .map_err(|e| Error::io("read", copy.path(), e))?;
            if before != after {
                return Err(Error::parse(
                    "lockfile",
                    workspace.root.join("Cargo.lock"),
                    "Cargo.lock is out of date with the manifests; update it first",
                ));
            }
        }
        Ok(state)
    }

    fn resolve_change(&self, project: &Path, change: &ProposedChange) -> Result<Candidate> {
        let workspace = self.supported_workspace(project)?;
        let args = add_arguments(change, &workspace)?;
        let workdir = isolate(&workspace)?;
        self.cargo(workdir.path(), &args)?;
        // The manifests changed, so read the copy, not the project.
        let changed = Workspace::discover(workdir.path())?;
        let state = lockfile_state(workdir.path(), &changed)?;
        Ok(Candidate {
            change: change.clone(),
            state,
            workdir,
        })
    }

    fn refine(&self, candidate: &mut Candidate) -> Result<()> {
        if candidate.state.precision == Precision::Full {
            return Ok(());
        }
        let dir = candidate.workdir.path().to_path_buf();
        self.refine_state(&dir, &mut candidate.state)
    }

    fn tool_invocations(&self) -> Vec<String> {
        self.invocations
            .lock()
            .map(|log| log.clone())
            .unwrap_or_default()
    }
}
