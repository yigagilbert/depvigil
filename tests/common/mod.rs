//! Helpers shared by the integration tests.
//!
//! Fixtures are never used in place: every test works on a temporary copy.
//! Tests that need a submodule fixture skip with a message when it is absent.

#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use depvigil::ecosystem::cargo::CargoEcosystem;
use depvigil::model::sha256_hex;

/// The file a fixture build script writes if it ever runs.
pub const BUILD_MARKER: &str = "BUILD_SCRIPT_RAN.marker";

const SKIPPED_DIRS: [&str; 3] = [".git", "target", ".depvigil"];

pub fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// A Cargo home kept under `target/` so that later runs, and the CI cache,
/// start warm. Tests never touch the developer's own Cargo home.
pub fn cargo_home() -> PathBuf {
    let home = repo_root().join("target").join("depvigil-test-cargo-home");
    fs::create_dir_all(&home).expect("create test cargo home");
    home
}

pub fn cargo() -> CargoEcosystem {
    CargoEcosystem::new().with_cargo_home(cargo_home())
}

/// The fixture directory, or `None` when it is a submodule that has not been
/// checked out.
pub fn fixture(name: &str) -> Option<PathBuf> {
    let dir = repo_root().join("fixtures").join(name);
    let present = match name {
        "advisory-db" => dir.join("crates").is_dir(),
        _ => dir.join("Cargo.toml").is_file(),
    };
    present.then_some(dir)
}

/// Returns the fixture or skips the calling test with a clear message.
#[macro_export]
macro_rules! require_fixture {
    ($name:expr) => {
        match $crate::common::fixture($name) {
            Some(dir) => dir,
            None => {
                eprintln!(
                    "SKIPPED: fixture `{}` is not checked out; run `git submodule update --init` to include this test",
                    $name
                );
                return;
            }
        }
    };
}

pub fn copy_tree(source: &Path, dest: &Path) {
    fs::create_dir_all(dest).expect("create dir");
    for entry in fs::read_dir(source).expect("read_dir") {
        let entry = entry.expect("entry");
        let name = entry.file_name();
        let file_type = entry.file_type().expect("file type");
        if file_type.is_dir() {
            if !SKIPPED_DIRS.iter().any(|s| name.to_str() == Some(s)) {
                copy_tree(&entry.path(), &dest.join(&name));
            }
        } else if file_type.is_file() {
            fs::copy(entry.path(), dest.join(&name)).expect("copy file");
        }
    }
}

/// A temporary directory holding a full copy of a fixture workspace.
pub struct Project {
    _temp: tempfile::TempDir,
    pub dir: PathBuf,
}

/// Copies a fixture into a fresh temporary directory.
pub fn project_from(fixture_dir: &Path) -> Project {
    let temp = tempfile::tempdir().expect("tempdir");
    let dir = temp.path().join("workspace");
    copy_tree(fixture_dir, &dir);
    Project { _temp: temp, dir }
}

/// SHA-256 of every file under `dir`, keyed by relative path. `.git`,
/// `target` and `.depvigil` are left out, so the result describes the
/// workspace itself.
pub fn tree_hashes(dir: &Path) -> BTreeMap<String, String> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, String>) {
        for entry in fs::read_dir(dir).expect("read_dir") {
            let entry = entry.expect("entry");
            let name = entry.file_name();
            let path = entry.path();
            if entry.file_type().expect("file type").is_dir() {
                if !SKIPPED_DIRS.iter().any(|s| name.to_str() == Some(s)) {
                    walk(root, &path, out);
                }
            } else {
                let relative = path
                    .strip_prefix(root)
                    .expect("under root")
                    .to_string_lossy()
                    .into_owned();
                out.insert(relative, sha256_hex(&fs::read(&path).expect("read file")));
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

/// Paths under `dir` that would prove something was compiled or a build
/// script ran: any `target` directory and any build marker file.
pub fn build_evidence(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        for entry in fs::read_dir(&current).expect("read_dir") {
            let entry = entry.expect("entry");
            let name = entry.file_name();
            let path = entry.path();
            if name.to_str() == Some("target") || name.to_str() == Some(BUILD_MARKER) {
                found.push(path);
            } else if entry.file_type().expect("file type").is_dir()
                && name.to_str() != Some(".git")
            {
                stack.push(path);
            }
        }
    }
    found
}

/// Runs Cargo directly, as ground truth, with the test Cargo home. Only ever
/// pointed at a temporary copy, and only with resolution subcommands.
pub fn cargo_ground_truth(dir: &Path, args: &[&str]) -> String {
    assert!(
        matches!(args[0], "add" | "metadata"),
        "ground truth may only resolve"
    );
    let mut command = Command::new("cargo");
    command
        .current_dir(dir)
        .args(args)
        .env("CARGO_HOME", cargo_home());
    for (key, _) in std::env::vars() {
        if key.starts_with("CARGO_MANIFEST_") || key.starts_with("CARGO_PKG_") {
            command.env_remove(key);
        }
    }
    let output = command.output().expect("run cargo");
    assert!(
        output.status.success(),
        "cargo {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("utf-8")
}

/// What `cargo metadata` says about a directory, in terms that do not depend
/// on depvigil's own code: resolved packages and edges by name and version.
pub struct GroundTruth {
    /// `name version` of every package in the resolve graph.
    pub packages: BTreeSet<String>,
    /// `from-name from-version -> to-name to-version (kind)`.
    pub edges: BTreeSet<String>,
    /// `name version` of every workspace member.
    pub members: BTreeSet<String>,
}

pub fn ground_truth(dir: &Path) -> GroundTruth {
    let stdout = cargo_ground_truth(dir, &["metadata", "--format-version", "1"]);
    let metadata: serde_json::Value = serde_json::from_str(&stdout).expect("metadata JSON");
    let mut names = BTreeMap::new();
    for package in metadata["packages"].as_array().expect("packages") {
        names.insert(
            package["id"].as_str().expect("id").to_string(),
            format!(
                "{} {}",
                package["name"].as_str().expect("name"),
                package["version"].as_str().expect("version")
            ),
        );
    }
    let mut truth = GroundTruth {
        packages: BTreeSet::new(),
        edges: BTreeSet::new(),
        members: BTreeSet::new(),
    };
    for node in metadata["resolve"]["nodes"].as_array().expect("nodes") {
        let from = &names[node["id"].as_str().expect("id")];
        truth.packages.insert(from.clone());
        for dep in node["deps"].as_array().expect("deps") {
            let to = &names[dep["pkg"].as_str().expect("pkg")];
            for kind in dep["dep_kinds"].as_array().expect("kinds") {
                let kind = kind["kind"].as_str().unwrap_or("normal");
                truth.edges.insert(format!("{from} -> {to} ({kind})"));
            }
        }
    }
    for member in metadata["workspace_members"].as_array().expect("members") {
        truth
            .members
            .insert(names[member.as_str().expect("member")].clone());
    }
    truth
}

/// `name version` from a Package URL such as `pkg:cargo/regex@1.5.4?source=…`.
pub fn name_version(purl: &str) -> String {
    let rest = purl.strip_prefix("pkg:cargo/").expect("cargo purl");
    let rest = rest.split('?').next().expect("purl body");
    let (name, version) = rest.split_once('@').expect("versioned purl");
    format!("{name} {version}")
}
