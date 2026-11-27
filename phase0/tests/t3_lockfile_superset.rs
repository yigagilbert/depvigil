//! Test 3 follow-up: the lockfile written by the change contains the
//! `cargo metadata` resolve set.
//!
//! The two-step simulation relies on this. Its fast first step reads only the
//! `Cargo.lock` that `cargo add` (or `cargo update --precise`) writes. That is
//! safe only if no package of the full resolve graph is missing from the
//! lockfile, so an advisory hit cannot be overlooked by the fast step.
//!
//! For every Test 3 case, in a manifest-only copy:
//!   1. apply the change and read the lockfile it wrote;
//!   2. run `cargo metadata` and read the resolve set;
//!   3. assert the resolve set is a subset of the lockfile set, and that
//!      `cargo metadata` left the lockfile unchanged.
//!
//! Packages are compared by name, version and source.
//!
//! Run: `cargo test --test t3_lockfile_superset -- --nocapture`

mod common;

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use common::cases::CASES;
use common::graph::shared_cargo_home;
use common::{add_stubs, copy_manifests, file_sha256, find_named, fixture, parse_metadata, run_cargo};

/// `(name, version, source)`; the source is empty for a path dependency.
type PackageSet = BTreeSet<(String, String, String)>;

fn lock_set(lock: &Path) -> PackageSet {
    let table: toml::Table = fs::read_to_string(lock)
        .expect("read Cargo.lock")
        .parse()
        .expect("parse Cargo.lock");
    table["package"]
        .as_array()
        .expect("package array")
        .iter()
        .map(|p| {
            let field = |key: &str| p.get(key).and_then(|v| v.as_str()).unwrap_or_default().to_string();
            (field("name"), field("version"), field("source"))
        })
        .collect()
}

fn resolve_set(metadata_stdout: &str, root: &Path) -> PackageSet {
    let metadata = parse_metadata(metadata_stdout, root);
    let nodes: BTreeSet<&str> = metadata["resolve"]["nodes"]
        .as_array()
        .expect("nodes")
        .iter()
        .map(|n| n["id"].as_str().expect("id"))
        .collect();
    metadata["packages"]
        .as_array()
        .expect("packages")
        .iter()
        .filter(|p| nodes.contains(p["id"].as_str().expect("id")))
        .map(|p| {
            let field = |key: &str| p[key].as_str().unwrap_or_default().to_string();
            (field("name"), field("version"), field("source"))
        })
        .collect()
}

#[test]
fn t3_lockfile_written_by_the_change_contains_the_resolve_set() {
    let home = shared_cargo_home();
    let mut failures = Vec::new();
    for case in CASES {
        let tag = format!("[t3 lockfile case {}]", case.id);
        let temp = tempfile::tempdir().expect("tempdir");
        let dir = temp.path();
        copy_manifests(&fixture(case.fixture), dir);
        add_stubs(dir);

        let change = run_cargo(dir, &home, case.change);
        assert!(change.success, "cargo {} failed: {}", case.change.join(" "), change.stderr);
        let lock = dir.join("Cargo.lock");
        let lock_packages = lock_set(&lock);
        let lock_sha = file_sha256(&lock);

        let metadata = run_cargo(dir, &home, &["metadata", "--format-version", "1"]);
        assert!(metadata.success, "cargo metadata failed: {}", metadata.stderr);
        let resolve_packages = resolve_set(&metadata.stdout, dir);
        assert!(find_named(dir, "target").is_empty(), "nothing may be compiled");

        let missing: Vec<String> = resolve_packages
            .difference(&lock_packages)
            .map(|(name, version, source)| format!("{name} {version} {source}"))
            .collect();
        let lock_only = lock_packages.difference(&resolve_packages).count();
        let lock_unchanged = file_sha256(&lock) == lock_sha;
        println!(
            "{tag} cargo {} (fixture `{}`): lockfile {} packages, resolve set {} packages, lockfile-only {}, missing from lockfile {}, lockfile unchanged by cargo metadata: {lock_unchanged}",
            case.change.join(" "),
            case.fixture,
            lock_packages.len(),
            resolve_packages.len(),
            lock_only,
            missing.len()
        );
        for package in &missing {
            println!("{tag}   in the resolve set but not in the lockfile: {package}");
        }
        if !missing.is_empty() {
            failures.push(format!("case {}: {} resolve packages missing from the lockfile", case.id, missing.len()));
        }
        if !lock_unchanged {
            failures.push(format!("case {}: cargo metadata rewrote the lockfile", case.id));
        }
    }
    assert!(failures.is_empty(), "lockfile does not contain the resolve set: {failures:?}");
}
