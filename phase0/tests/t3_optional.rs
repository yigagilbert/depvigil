//! Test 3, optional measurements. These are not pass criteria.
//!
//! For case 1 (`small`) and case 5 (`public`), each variant runs in its own
//! manifest-only copy with its own empty `CARGO_HOME`:
//!
//!   metadata            cargo add, then cargo metadata (the Test 3 method)
//!   filter-platform     cargo add, then cargo metadata --filter-platform <host>
//!   add-only            cargo add, then read the Cargo.lock that it wrote
//!   generate-lockfile   cargo add, then cargo generate-lockfile, read Cargo.lock
//!
//! Run: `cargo test --test t3_optional -- --nocapture`

mod common;

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::Instant;

use common::{add_stubs, cached_crates, copy_manifests, fixture, parse_metadata, run_cargo};

fn dir_size(path: &Path) -> u64 {
    if !path.exists() {
        return 0;
    }
    let meta = fs::symlink_metadata(path).expect("metadata");
    if !meta.is_dir() {
        return meta.len();
    }
    fs::read_dir(path)
        .expect("read_dir")
        .map(|entry| dir_size(&entry.expect("entry").path()))
        .sum()
}

fn host_triple() -> String {
    let output = Command::new("rustc").arg("-vV").output().expect("rustc -vV");
    String::from_utf8(output.stdout)
        .expect("utf-8")
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .expect("host line")
        .to_string()
}

/// `name version` for every `[[package]]` in a lockfile.
fn lock_packages(lock: &Path) -> BTreeSet<String> {
    let table: toml::Table = fs::read_to_string(lock)
        .expect("read Cargo.lock")
        .parse()
        .expect("parse Cargo.lock");
    table["package"]
        .as_array()
        .expect("package array")
        .iter()
        .map(|p| {
            format!(
                "{} {}",
                p["name"].as_str().expect("name"),
                p["version"].as_str().expect("version")
            )
        })
        .collect()
}

/// `name version` for every node of a `cargo metadata` resolve graph.
fn metadata_packages(stdout: &str, root: &Path) -> BTreeSet<String> {
    let metadata = parse_metadata(stdout, root);
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
            format!(
                "{} {}",
                p["name"].as_str().expect("name"),
                p["version"].as_str().expect("version")
            )
        })
        .collect()
}

struct Measurement {
    packages: BTreeSet<String>,
}

fn measure(tag: &str, variant: &str, fixture_name: &str, change: &[&str], host: &str) -> Measurement {
    let fixture_root = fixture(fixture_name);
    let home = tempfile::tempdir().expect("cargo home");
    let home = home.path();
    let temp = tempfile::tempdir().expect("tempdir");
    let dir = temp.path();

    let start = Instant::now();
    copy_manifests(&fixture_root, dir);
    add_stubs(dir);
    let added = run_cargo(dir, home, change);
    assert!(added.success, "cargo add failed: {}", added.stderr);

    let packages = match variant {
        "metadata" => {
            let run = run_cargo(dir, home, &["metadata", "--format-version", "1"]);
            assert!(run.success, "{}", run.stderr);
            metadata_packages(&run.stdout, dir)
        }
        "filter-platform" => {
            let run = run_cargo(
                dir,
                home,
                &["metadata", "--format-version", "1", "--filter-platform", host],
            );
            assert!(run.success, "{}", run.stderr);
            metadata_packages(&run.stdout, dir)
        }
        "add-only" => lock_packages(&dir.join("Cargo.lock")),
        "generate-lockfile" => {
            let run = run_cargo(dir, home, &["generate-lockfile"]);
            assert!(run.success, "{}", run.stderr);
            lock_packages(&dir.join("Cargo.lock"))
        }
        other => panic!("unknown variant {other}"),
    };
    let wall = start.elapsed();

    let registry = home.join("registry");
    println!(
        "{tag} {variant}: wall {wall:?}; packages {}; crate archives {} ({} bytes); index cache {} bytes; unpacked sources {} bytes",
        packages.len(),
        cached_crates(home).len(),
        dir_size(&registry.join("cache")),
        dir_size(&registry.join("index")),
        dir_size(&registry.join("src")),
    );
    assert!(common::find_named(dir, "target").is_empty(), "nothing may be compiled");
    Measurement { packages }
}

fn report_difference(tag: &str, name: &str, other: &BTreeSet<String>, baseline: &BTreeSet<String>) {
    let extra: Vec<&str> = other.difference(baseline).map(String::as_str).collect();
    let missing: Vec<&str> = baseline.difference(other).map(String::as_str).collect();
    println!(
        "{tag} {name} vs metadata: {} only in {name}, {} only in metadata",
        extra.len(),
        missing.len()
    );
    if !extra.is_empty() {
        println!("{tag}   only in {name}: {}", extra.join(", "));
    }
    if !missing.is_empty() {
        println!("{tag}   only in metadata: {}", missing.join(", "));
    }
}

#[test]
fn t3_optional_lockfile_only_and_filter_platform() {
    let host = host_triple();
    println!("[t3 optional] host triple: {host}");
    let cases: [(&str, &str, &[&str]); 2] = [
        ("1", "small", &["add", "getrandom@=0.2.15", "-p", "small-app"]),
        ("5", "public", &["add", "petgraph@=0.6.5"]),
    ];
    for (id, fixture_name, change) in cases {
        let tag = format!("[t3 optional case {id}]");
        println!(
            "{tag} fixture `{fixture_name}`, original Cargo.lock packages: {}",
            lock_packages(&fixture(fixture_name).join("Cargo.lock")).len()
        );
        let metadata = measure(&tag, "metadata", fixture_name, change, &host);
        let filtered = measure(&tag, "filter-platform", fixture_name, change, &host);
        let add_only = measure(&tag, "add-only", fixture_name, change, &host);
        let regenerated = measure(&tag, "generate-lockfile", fixture_name, change, &host);
        report_difference(&tag, "filter-platform", &filtered.packages, &metadata.packages);
        report_difference(&tag, "add-only", &add_only.packages, &metadata.packages);
        report_difference(&tag, "generate-lockfile", &regenerated.packages, &metadata.packages);
        report_difference(&tag, "generate-lockfile-vs-add-only", &regenerated.packages, &add_only.packages);
    }
}
