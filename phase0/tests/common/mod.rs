//! Helpers shared by the Phase 0 tests: fixture paths, manifest-only copies,
//! safe Cargo invocations and resolve-graph comparison.
//!
//! Safety limit from the brief: only `cargo add`, `cargo update`,
//! `cargo generate-lockfile`, `cargo metadata` and `cargo tree` may run
//! against a candidate dependency set. `run_cargo` enforces that list.

#![allow(dead_code)]

pub mod cases;
pub mod graph;

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value as Json;
use sha2::{Digest, Sha256};

pub const BUILD_MARKER: &str = "BUILD_SCRIPT_RAN.marker";
const ALLOWED_CARGO: [&str; 5] = ["add", "update", "generate-lockfile", "metadata", "tree"];
const SKIPPED_DIRS: [&str; 2] = [".git", "target"];

pub fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(name)
        .canonicalize()
        .expect("fixture exists")
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub fn file_sha256(path: &Path) -> String {
    sha256_hex(&fs::read(path).expect("read file to hash"))
}

/// `git status --porcelain` limited to `dir`. Empty output means clean.
pub fn git_status(dir: &Path) -> String {
    let output = Command::new("git")
        .current_dir(dir)
        .args(["status", "--porcelain", "--ignored", "--", "."])
        .output()
        .expect("run git status");
    assert!(output.status.success(), "git status failed");
    String::from_utf8(output.stdout).expect("utf-8 git output")
}

/// Every path under `root` with the given file name, skipping `.git` and
/// `target` directories.
pub fn find_named(root: &Path, name: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).expect("read_dir") {
            let entry = entry.expect("dir entry");
            let path = entry.path();
            let file_name = entry.file_name();
            let file_type = entry.file_type().expect("file type");
            if file_name.to_str() == Some(name) {
                found.push(path.clone());
            }
            if file_type.is_dir() && !SKIPPED_DIRS.iter().any(|s| file_name.to_str() == Some(s)) {
                stack.push(path);
            }
        }
    }
    found.sort();
    found
}

fn copy_file(from_root: &Path, to_root: &Path, file: &Path) {
    let relative = file.strip_prefix(from_root).expect("file under root");
    let target = to_root.join(relative);
    fs::create_dir_all(target.parent().expect("parent")).expect("create dirs");
    fs::copy(file, target).expect("copy file");
}

/// The method under test: copy only the manifests of `source` into `dest`.
/// That is every `Cargo.toml`, the `Cargo.lock` files and any
/// `.cargo/config.toml`. No source file and no build script is copied.
/// Returns the number of files copied.
pub fn copy_manifests(source: &Path, dest: &Path) -> usize {
    let mut copied = 0;
    for name in ["Cargo.toml", "Cargo.lock"] {
        for file in find_named(source, name) {
            copy_file(source, dest, &file);
            copied += 1;
        }
    }
    for cargo_dir in find_named(source, ".cargo") {
        let config = cargo_dir.join("config.toml");
        if config.is_file() {
            copy_file(source, dest, &config);
            copied += 1;
        }
    }
    copied
}

/// Ground truth: a full copy of the fixture's working tree, without `.git`
/// and `target`.
pub fn copy_full(source: &Path, dest: &Path) {
    let mut stack = vec![source.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).expect("read_dir") {
            let entry = entry.expect("dir entry");
            let path = entry.path();
            let file_type = entry.file_type().expect("file type");
            if file_type.is_dir() {
                if !SKIPPED_DIRS.iter().any(|s| entry.file_name().to_str() == Some(s)) {
                    stack.push(path);
                }
            } else if file_type.is_file() {
                copy_file(source, dest, &path);
            }
        }
    }
}

/// Adds empty source stubs for every package manifest under `root` so Cargo
/// accepts manifests whose sources were not copied. Explicit target paths
/// (`[lib] path`, `[[bin]] path` and similar, and `build = "..."`) get an
/// empty file each; a package with no explicit library path also gets an
/// empty `src/lib.rs`. Returns the stub paths created, relative to `root`.
pub fn add_stubs(root: &Path) -> Vec<String> {
    let mut created = Vec::new();
    for manifest_path in find_named(root, "Cargo.toml") {
        let text = fs::read_to_string(&manifest_path).expect("read manifest");
        let Ok(manifest) = text.parse::<toml::Table>() else {
            continue; // not a Cargo manifest we can read, such as test data
        };
        let Some(package) = manifest.get("package").and_then(|p| p.as_table()) else {
            continue; // virtual manifest
        };
        let package_dir = manifest_path.parent().expect("manifest dir");

        let mut stubs: Vec<String> = Vec::new();
        let lib_path = manifest
            .get("lib")
            .and_then(|lib| lib.get("path"))
            .and_then(|p| p.as_str());
        stubs.push(lib_path.unwrap_or("src/lib.rs").to_string());
        for kind in ["bin", "example", "test", "bench"] {
            let targets = manifest.get(kind).and_then(|t| t.as_array());
            for target in targets.into_iter().flatten() {
                if let Some(path) = target.get("path").and_then(|p| p.as_str()) {
                    stubs.push(path.to_string());
                }
            }
        }
        if let Some(build) = package.get("build").and_then(|b| b.as_str()) {
            stubs.push(build.to_string());
        }

        for stub in stubs {
            let stub_path = package_dir.join(&stub);
            if !stub_path.exists() {
                fs::create_dir_all(stub_path.parent().expect("parent")).expect("create dirs");
                fs::write(&stub_path, "").expect("write stub");
                let relative = stub_path.strip_prefix(root).expect("under root");
                created.push(relative.display().to_string());
            }
        }
    }
    created.sort();
    created
}

pub struct CargoRun {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Runs an allowed Cargo subcommand in `dir` with its own `CARGO_HOME`.
/// The `CARGO_*` and `RUST*` variables that `cargo test` sets for this
/// process are removed so they cannot leak into the candidate resolution.
pub fn run_cargo(dir: &Path, cargo_home: &Path, args: &[&str]) -> CargoRun {
    let subcommand = args.first().expect("cargo subcommand");
    assert!(
        ALLOWED_CARGO.contains(subcommand),
        "cargo {subcommand} is not allowed on a candidate dependency set"
    );
    let mut command = Command::new("cargo");
    command.current_dir(dir).args(args);
    for (key, _) in std::env::vars() {
        if key.starts_with("CARGO_") || key.starts_with("RUSTC") || key == "RUSTFLAGS" || key == "CARGO" {
            command.env_remove(key);
        }
    }
    command.env("CARGO_HOME", cargo_home);
    command.env("CARGO_TERM_COLOR", "never");
    let output = command.output().expect("spawn cargo");
    CargoRun {
        success: output.status.success(),
        stdout: String::from_utf8(output.stdout).expect("utf-8 stdout"),
        stderr: String::from_utf8(output.stderr).expect("utf-8 stderr"),
    }
}

/// The `Downloaded <crate> v<version>` lines Cargo printed, as `crate v1.2.3`.
pub fn downloaded(stderr: &str) -> Vec<String> {
    stderr
        .lines()
        .filter_map(|line| line.trim().strip_prefix("Downloaded "))
        .filter(|rest| !rest.contains(" crates ("))
        .map(|rest| rest.split(" (").next().unwrap_or(rest).to_string())
        .collect()
}

/// `.crate` archives present in a Cargo home's registry cache.
pub fn cached_crates(cargo_home: &Path) -> BTreeSet<String> {
    let cache = cargo_home.join("registry").join("cache");
    let mut crates = BTreeSet::new();
    if cache.is_dir() {
        for index in fs::read_dir(cache).expect("read cache") {
            for file in fs::read_dir(index.expect("entry").path()).expect("read index cache") {
                let name = file.expect("entry").file_name();
                crates.insert(name.to_string_lossy().trim_end_matches(".crate").to_string());
            }
        }
    }
    crates
}

/// A resolve graph with machine-specific paths removed, ready to compare.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Resolve {
    /// Package ids of every resolved package.
    pub packages: BTreeSet<String>,
    /// `from -> to [name] (kinds)` for every resolved dependency edge.
    pub edges: BTreeSet<String>,
    /// `package: feature,feature` for every resolved package.
    pub features: BTreeSet<String>,
    pub workspace_members: BTreeSet<String>,
}

/// Parses `cargo metadata` output and replaces the absolute path of `root`
/// with `<ROOT>` everywhere, so two checkouts in different directories
/// compare equal.
pub fn parse_metadata(stdout: &str, root: &Path) -> Json {
    let root = root.canonicalize().expect("canonical root");
    let root = root.to_str().expect("utf-8 root");
    serde_json::from_str(&stdout.replace(root, "<ROOT>")).expect("cargo metadata JSON")
}

pub fn resolve_of(metadata: &Json) -> Resolve {
    let strings = |value: &Json| -> BTreeSet<String> {
        value
            .as_array()
            .expect("array")
            .iter()
            .map(|v| v.as_str().expect("string").to_string())
            .collect()
    };
    let mut resolve = Resolve {
        packages: BTreeSet::new(),
        edges: BTreeSet::new(),
        features: BTreeSet::new(),
        workspace_members: strings(&metadata["workspace_members"]),
    };
    for node in metadata["resolve"]["nodes"].as_array().expect("resolve nodes") {
        let id = node["id"].as_str().expect("node id");
        resolve.packages.insert(id.to_string());
        let features: Vec<String> = strings(&node["features"]).into_iter().collect();
        resolve.features.insert(format!("{id}: {}", features.join(",")));
        for dep in node["deps"].as_array().expect("deps") {
            let mut kinds: Vec<String> = dep["dep_kinds"]
                .as_array()
                .expect("dep_kinds")
                .iter()
                .map(|k| {
                    format!(
                        "{}@{}",
                        k["kind"].as_str().unwrap_or("normal"),
                        k["target"].as_str().unwrap_or("any")
                    )
                })
                .collect();
            kinds.sort();
            resolve.edges.insert(format!(
                "{id} -> {} [{}] ({})",
                dep["pkg"].as_str().expect("dep pkg"),
                dep["name"].as_str().expect("dep name"),
                kinds.join(";")
            ));
        }
    }
    resolve
}

/// Lines describing how two resolve graphs differ. Empty when identical.
pub fn diff(label_a: &str, a: &Resolve, label_b: &str, b: &Resolve) -> Vec<String> {
    let mut lines = Vec::new();
    let mut compare = |what: &str, x: &BTreeSet<String>, y: &BTreeSet<String>| {
        for only in x.difference(y) {
            lines.push(format!("{what} only in {label_a}: {only}"));
        }
        for only in y.difference(x) {
            lines.push(format!("{what} only in {label_b}: {only}"));
        }
    };
    compare("package", &a.packages, &b.packages);
    compare("edge", &a.edges, &b.edges);
    compare("features", &a.features, &b.features);
    compare("workspace member", &a.workspace_members, &b.workspace_members);
    lines
}
