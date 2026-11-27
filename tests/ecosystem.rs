//! The Cargo ecosystem: reading the current state, isolated resolution of a
//! proposed change, and unsupported inputs.
//!
//! Migrated from Phase 0 Test 3 (isolated candidate resolution) and its
//! lockfile-superset follow-up.

mod common;

use std::collections::BTreeSet;
use std::fs;

use common::{
    build_evidence, cargo, cargo_ground_truth, ground_truth, name_version, project_from,
    tree_hashes,
};
use depvigil::ecosystem::Ecosystem;
use depvigil::model::{EdgeKind, Precision, ProposedChange, ResolvedState, UnsupportedInput};
use depvigil::Error;

fn resolve_edges(state: &ResolvedState) -> BTreeSet<String> {
    state
        .edges
        .iter()
        .filter(|edge| edge.kind != EdgeKind::Lock)
        .map(|edge| {
            format!(
                "{} -> {} ({})",
                name_version(&edge.from),
                name_version(&edge.to),
                edge.kind.as_str()
            )
        })
        .collect()
}

fn active(state: &ResolvedState) -> BTreeSet<String> {
    state
        .packages
        .iter()
        .filter(|p| p.active == Some(true))
        .map(|p| name_version(&p.purl))
        .collect()
}

fn change(spec: &str, member: Option<&str>, features: &[&str]) -> ProposedChange {
    let mut change = ProposedChange::parse(spec).expect("valid spec");
    change.member = member.map(str::to_string);
    change.features = features.iter().map(|f| f.to_string()).collect();
    change
}

#[test]
fn current_state_of_small_matches_cargo_metadata() {
    let fixture = require_fixture!("small");
    let project = project_from(&fixture);
    let before = tree_hashes(&project.dir);
    let cargo = cargo();

    let lock = cargo
        .current_state(&project.dir, Precision::Lockfile)
        .expect("lockfile state");
    assert_eq!(lock.packages.len(), 15);
    assert!(lock.packages.iter().all(|p| p.active.is_none()));
    assert!(
        cargo.tool_invocations().is_empty(),
        "a lockfile read runs no Cargo command"
    );
    assert_eq!(
        lock.roots,
        [
            "pkg:cargo/small-app@0.1.0?source=path:app",
            "pkg:cargo/small-core@0.1.0?source=path:core"
        ]
    );
    assert!(lock
        .purls()
        .contains("pkg:cargo/marker@0.1.0?source=path:vendor/marker"));

    let full = cargo
        .current_state(&project.dir, Precision::Full)
        .expect("full state");
    let truth = ground_truth(&project_from(&fixture).dir);
    assert_eq!(active(&full), truth.packages);
    assert_eq!(resolve_edges(&full), truth.edges);
    let roots: BTreeSet<String> = full.roots.iter().map(|r| name_version(r)).collect();
    assert_eq!(roots, truth.members);
    assert_eq!(
        full.roots, lock.roots,
        "members found from manifests equal Cargo's"
    );

    // One workspace, one digest, whichever way it was read.
    assert_eq!(lock.digest(), full.digest());
    let canonical = full.canonical();
    assert!(!canonical.contains(project.dir.to_str().unwrap()));
    assert!(!canonical.contains("file://"));
    assert!(canonical.contains("declared app normal - regex package=regex req==1.5.4"));
    assert!(canonical.contains("resolver 2"));

    assert_eq!(
        tree_hashes(&project.dir),
        before,
        "the project is never written to"
    );
    assert!(build_evidence(&project.dir).is_empty());
}

#[test]
fn current_state_of_bat_matches_cargo_metadata() {
    let fixture = require_fixture!("bat");
    let project = project_from(&fixture);
    let full = cargo()
        .current_state(&project.dir, Precision::Full)
        .expect("full state");
    let truth = ground_truth(&project_from(&fixture).dir);
    assert_eq!(active(&full), truth.packages);
    assert_eq!(resolve_edges(&full), truth.edges);
    let inactive = full
        .packages
        .iter()
        .filter(|p| p.active == Some(false))
        .count();
    println!(
        "bat: {} lockfile packages, {} active, {} inactive, {} resolve edges",
        full.packages.len(),
        truth.packages.len(),
        inactive,
        truth.edges.len()
    );
    assert_eq!(full.packages.len(), 208);
    assert_eq!(inactive, 9);
}

/// Isolated resolution equals a full checkout with the same change applied.
fn assert_isolated_equals_ground_truth(
    fixture: &std::path::Path,
    proposed: &ProposedChange,
    add_args: &[&str],
) {
    let project = project_from(fixture);
    let before = tree_hashes(&project.dir);
    let cargo = cargo();

    let mut candidate = cargo
        .resolve_change(&project.dir, proposed)
        .expect("resolve change");
    assert_eq!(candidate.state.precision, Precision::Lockfile);
    assert!(
        !cargo
            .tool_invocations()
            .iter()
            .any(|c| c.starts_with("cargo metadata")),
        "the fast step must not run cargo metadata"
    );
    let lock_after_add =
        fs::read(candidate.workdir().join("Cargo.lock")).expect("candidate lockfile");
    let lock_digest = candidate.state.digest();

    cargo.refine(&mut candidate).expect("refine");
    assert_eq!(candidate.state.precision, Precision::Full);
    assert_eq!(
        candidate.state.digest(),
        lock_digest,
        "refining does not change the digest"
    );
    assert_eq!(
        fs::read(candidate.workdir().join("Cargo.lock")).expect("candidate lockfile"),
        lock_after_add,
        "cargo metadata leaves the lockfile written by cargo add unchanged"
    );

    // Ground truth: the same change on a full copy of the fixture.
    let truth_project = project_from(fixture);
    cargo_ground_truth(&truth_project.dir, add_args);
    let truth = ground_truth(&truth_project.dir);
    assert_eq!(
        active(&candidate.state),
        truth.packages,
        "resolved packages"
    );
    assert_eq!(
        resolve_edges(&candidate.state),
        truth.edges,
        "resolved edges"
    );
    assert_eq!(
        lock_after_add,
        fs::read(truth_project.dir.join("Cargo.lock")).expect("truth lockfile"),
        "identical Cargo.lock"
    );

    // The lockfile contains the whole resolve set (ADR 0006).
    let lock_packages: BTreeSet<String> = candidate
        .state
        .packages
        .iter()
        .map(|p| name_version(&p.purl))
        .collect();
    assert!(truth.packages.is_subset(&lock_packages));

    // Safety: nothing built, no build script ran, the project is untouched.
    assert!(
        build_evidence(candidate.workdir()).is_empty(),
        "nothing compiled in the copy"
    );
    assert!(build_evidence(&truth_project.dir).is_empty());
    assert_eq!(tree_hashes(&project.dir), before, "project unchanged");
    assert!(build_evidence(&project.dir).is_empty());
}

#[test]
fn isolated_resolution_equals_ground_truth_on_small() {
    let fixture = require_fixture!("small");
    // Phase 0 Test 3, cases 1 to 4.
    assert_isolated_equals_ground_truth(
        &fixture,
        &change("getrandom@=0.2.15", Some("small-app"), &[]),
        &["add", "getrandom@=0.2.15", "-p", "small-app"],
    );
    assert_isolated_equals_ground_truth(
        &fixture,
        &change("smallvec@=1.6.1", Some("small-app"), &[]),
        &["add", "smallvec@=1.6.1", "-p", "small-app"],
    );
    assert_isolated_equals_ground_truth(
        &fixture,
        &change("serde@=1.0.210", Some("small-app"), &["derive"]),
        &[
            "add",
            "serde@=1.0.210",
            "-p",
            "small-app",
            "--features",
            "derive",
        ],
    );
    assert_isolated_equals_ground_truth(
        &fixture,
        &change("either@=1.13.0", Some("small-core"), &[]),
        &["add", "either@=1.13.0", "-p", "small-core"],
    );
}

#[test]
fn isolated_resolution_equals_ground_truth_on_bat() {
    let fixture = require_fixture!("bat");
    // Phase 0 Test 3, case 5.
    assert_isolated_equals_ground_truth(
        &fixture,
        &change("petgraph@=0.6.5", None, &[]),
        &["add", "petgraph@=0.6.5"],
    );
}

#[test]
fn a_workspace_with_several_members_needs_a_member() {
    let fixture = require_fixture!("small");
    let project = project_from(&fixture);
    let error = cargo()
        .resolve_change(&project.dir, &change("either", None, &[]))
        .unwrap_err();
    assert!(matches!(error, Error::InvalidInput(_)), "{error}");
    let error = cargo()
        .resolve_change(&project.dir, &change("either", Some("nope"), &[]))
        .unwrap_err();
    assert!(matches!(error, Error::InvalidInput(_)), "{error}");
}

#[test]
fn invalid_specs_are_refused_before_any_command_runs() {
    for spec in ["", "--config=x", "name@", "name@ --offline", "na me"] {
        assert!(
            ProposedChange::parse(spec).is_err(),
            "`{spec}` must be refused"
        );
    }
    assert!(ProposedChange::parse("serde_json@=1.0.1").is_ok());
}

/// Writes a one-package workspace with the given extra manifest text.
fn tiny_workspace(extra: &str) -> tempfile::TempDir {
    let temp = tempfile::tempdir().expect("tempdir");
    let manifest =
        format!("[package]\nname = \"tiny\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n{extra}");
    fs::write(temp.path().join("Cargo.toml"), manifest).expect("manifest");
    fs::write(
        temp.path().join("Cargo.lock"),
        "version = 4\n\n[[package]]\nname = \"tiny\"\nversion = \"0.1.0\"\n",
    )
    .expect("lockfile");
    fs::create_dir_all(temp.path().join("src")).expect("src");
    fs::write(temp.path().join("src/lib.rs"), "").expect("lib");
    temp
}

#[test]
fn unsupported_inputs_are_detected_and_refused() {
    let cargo = cargo();
    let cases = [
        (
            "[dependencies]\nleft = { git = \"https://example.invalid/left.git\" }\n",
            UnsupportedInput::GitDependency {
                manifest: "Cargo.toml".into(),
                name: "left".into(),
            },
        ),
        (
            "[patch.crates-io]\nserde = { path = \"vendor/serde\" }\n",
            UnsupportedInput::PatchSection {
                file: "Cargo.toml".into(),
            },
        ),
        (
            "[dependencies]\nsibling = { path = \"../sibling\" }\n",
            UnsupportedInput::PathOutsideRoot {
                manifest: "Cargo.toml".into(),
                name: "sibling".into(),
                path: "../sibling".into(),
            },
        ),
    ];
    for (extra, expected) in cases {
        let workspace = tiny_workspace(extra);
        assert_eq!(
            cargo.unsupported_inputs(workspace.path()).expect("scan"),
            vec![expected.clone()]
        );
        for result in [
            cargo
                .current_state(workspace.path(), Precision::Lockfile)
                .map(|_| ()),
            cargo
                .resolve_change(workspace.path(), &change("either", None, &[]))
                .map(|_| ()),
        ] {
            match result {
                Err(Error::Unsupported(inputs)) => assert_eq!(inputs, vec![expected.clone()]),
                other => panic!("expected an unsupported-input error, got {other:?}"),
            }
        }
    }
    assert!(
        cargo.tool_invocations().is_empty(),
        "an unsupported workspace is refused before any Cargo command runs"
    );
    let supported = tiny_workspace("");
    assert!(cargo
        .unsupported_inputs(supported.path())
        .expect("scan")
        .is_empty());
}
