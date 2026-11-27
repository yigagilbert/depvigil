//! depvigil governs its own dependencies (acceptance item 8).
//!
//! The project's own `.depvigil` directory is committed. These tests keep it
//! honest: it must describe the workspace as it is, agree with its text log,
//! and carry nothing that identifies a machine or a person.

mod common;

use std::fs;
use std::path::Path;

use common::{cargo, repo_root};
use depvigil::ecosystem::Ecosystem;
use depvigil::engine::DecisionStatus;
use depvigil::mirror;
use depvigil::model::Precision;
use depvigil::store::Store;

fn files_under(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in fs::read_dir(dir).expect("read_dir") {
        let path = entry.expect("entry").path();
        if path.is_dir() {
            files_under(&path, out);
        } else {
            out.push(path);
        }
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

#[test]
fn the_committed_store_matches_the_workspace() {
    let root = repo_root();
    // Reading the lockfile and manifests runs no Cargo command.
    let current = cargo()
        .current_state(&root, Precision::Lockfile)
        .expect("current state");
    let mut store = Store::open_read_only(&root).expect("the project's own store is committed");
    let baseline = store
        .baseline()
        .expect("baseline")
        .expect("depvigil init has been run");
    assert_eq!(
        baseline.digest,
        current.digest(),
        "the dependency set changed without depvigil: run `depvigil check <crate>` before adding a \
         dependency, then `depvigil init`, and commit .depvigil"
    );
    assert_eq!(
        store.project_name().expect("name").as_deref(),
        Some("depvigil")
    );
}

#[test]
fn every_dependency_added_since_the_baseline_was_approved_first() {
    let root = repo_root();
    let mut store = Store::open_read_only(&root).expect("open");
    let decisions = store.decisions().expect("decisions");
    assert!(
        !decisions.is_empty(),
        "at least one dependency went through depvigil check"
    );
    for decision in decisions.iter().filter(|d| d.verdict == "approved") {
        println!(
            "{} {} ({})",
            decision.id,
            decision.change,
            DecisionStatus::of(decision).as_str()
        );
    }
    // The current baseline is a state some approved decision produced.
    let baseline = store.baseline().expect("baseline").expect("has baseline");
    assert!(
        decisions
            .iter()
            .any(|d| d.verdict == "approved" && d.digest == baseline.digest),
        "the current dependency set is not the result of an approved decision"
    );
}

#[test]
fn the_text_log_agrees_with_the_committed_store() {
    let root = repo_root();
    let mut store = Store::open_read_only(&root).expect("open");
    let decisions = store.decisions().expect("decisions");
    let invalidations = store.invalidations().expect("invalidations");
    let lines = mirror::read(&root).expect("mirror");
    let logged = |kind: &str| {
        lines
            .iter()
            .filter(|l| l["type"] == kind)
            .collect::<Vec<_>>()
    };

    let decision_lines = logged("decision");
    assert_eq!(decision_lines.len(), decisions.len());
    for (line, decision) in decision_lines.iter().zip(&decisions) {
        assert_eq!(line["id"], decision.id);
        assert_eq!(line["verdict"], decision.verdict);
        assert_eq!(line["digest"], decision.digest);
        assert_eq!(line["decided_at"], decision.decided_at);
        assert_eq!(line["change"], decision.change);
    }
    let invalidation_lines = logged("invalidation");
    assert_eq!(invalidation_lines.len(), invalidations.len());
    for line in invalidation_lines {
        assert!(invalidations
            .iter()
            .any(|i| line["id"] == i.id && line["decision"] == i.decision));
    }
}

#[test]
fn the_committed_store_identifies_no_machine_or_person() {
    let root = repo_root();
    let mut files = Vec::new();
    files_under(&root.join(".depvigil"), &mut files);
    assert!(!files.is_empty());

    let mut forbidden: Vec<Vec<u8>> = [
        "/Users/",
        "/home/",
        "/private/",
        "/var/folders/",
        "/tmp/",
        "C:\\Users",
    ]
    .iter()
    .map(|s| s.as_bytes().to_vec())
    .collect();
    // Whoever runs the test: their home directory and this checkout's path.
    if let Some(home) = std::env::var_os("HOME") {
        forbidden.push(home.to_string_lossy().as_bytes().to_vec());
    }
    forbidden.push(root.to_string_lossy().as_bytes().to_vec());
    // Credential shapes. Plain words such as "secret" are not searched for:
    // advisory summaries use them legitimately.
    for marker in [
        "BEGIN PRIVATE KEY",
        "BEGIN OPENSSH PRIVATE KEY",
        "Authorization: ",
        "ghp_",
        "github_pat_",
        "AKIA",
    ] {
        forbidden.push(marker.as_bytes().to_vec());
    }
    if let Some(user) = std::env::var_os("USER").filter(|user| user.len() >= 5) {
        forbidden.push(user.to_string_lossy().as_bytes().to_vec());
    }

    for file in &files {
        let bytes = fs::read(file).expect("read store file");
        for needle in &forbidden {
            assert!(
                !contains(&bytes, needle),
                "{} contains `{}`",
                file.strip_prefix(&root).unwrap().display(),
                String::from_utf8_lossy(needle)
            );
        }
    }
    println!("scanned {} files under .depvigil", files.len());
}
