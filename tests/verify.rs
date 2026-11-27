//! `depvigil verify`: approval binding and drift detection. Covers
//! acceptance item 4 and migrates Phase 0 Test 5.

mod common;

use std::path::Path;

use common::{cargo_ground_truth, copy_tree, engine_at, project_from, run_cli, Project};
use depvigil::engine::DecisionStatus;
use depvigil::model::ProposedChange;
use depvigil::Error;

const T0: &str = "2026-11-27T09:00:00Z";
const T1: &str = "2026-11-27T10:00:00Z";

/// A copy of `small` with `either 1.13.0 -> small-core` approved.
fn approved(fixture: &Path) -> Project {
    let project = project_from(fixture);
    engine_at(&project.dir, T0).init().expect("init");
    let mut change = ProposedChange::parse("either@=1.13.0").expect("spec");
    change.member = Some("small-core".to_string());
    let report = engine_at(&project.dir, T1).check(&[change]).expect("check");
    assert_eq!(report.candidates[0].verdict, "approved");
    project
}

fn apply(project: &Project, args: &[&str]) {
    cargo_ground_truth(&project.dir, args);
}

fn describe(differences: &[depvigil::engine::Difference]) -> Vec<String> {
    differences
        .iter()
        .map(|d| format!("{} only in {}: {}", d.kind, d.only_in, d.detail))
        .collect()
}

#[test]
fn before_the_change_is_applied_the_workspace_does_not_match() {
    let fixture = require_fixture!("small");
    let project = approved(&fixture);
    let report = engine_at(&project.dir, T1).verify().expect("verify");
    assert!(!report.matches);
    assert!(describe(&report.differences)
        .iter()
        .any(|d| d.starts_with("package only in approved: either 1.13.0 ")));
}

#[test]
fn exact_application_matches() {
    let fixture = require_fixture!("small");
    let project = approved(&fixture);
    apply(&project, &["add", "either@=1.13.0", "-p", "small-core"]);
    let engine = engine_at(&project.dir, T1);
    let report = engine.verify().expect("verify");
    assert!(report.matches, "{:?}", describe(&report.differences));
    assert_eq!(report.approved_digest, report.current_digest);
    assert!(report.differences.is_empty());
    assert_eq!(report.decision, "D0001");
    assert_eq!(report.status, DecisionStatus::Valid);
    use depvigil::ecosystem::Ecosystem;
    assert!(
        engine.ecosystem().tool_invocations().is_empty(),
        "verify runs no Cargo command"
    );
}

#[test]
fn a_different_version_mismatches_and_is_named() {
    let fixture = require_fixture!("small");
    let project = approved(&fixture);
    apply(&project, &["add", "either@=1.12.0", "-p", "small-core"]);
    let report = engine_at(&project.dir, T1).verify().expect("verify");
    assert!(!report.matches);
    let differences = describe(&report.differences);
    println!("{differences:#?}");
    assert!(differences
        .iter()
        .any(|d| d.starts_with("package only in workspace: either 1.12.0 ")));
    assert!(differences
        .iter()
        .any(|d| d.starts_with("package only in approved: either 1.13.0 ")));
    assert!(differences.iter().any(|d| d
        .contains("declared only in workspace: core normal - either")
        && d.contains("req==1.12.0")));
}

#[test]
fn an_extra_crate_mismatches_and_is_named() {
    let fixture = require_fixture!("small");
    let project = approved(&fixture);
    apply(&project, &["add", "either@=1.13.0", "-p", "small-core"]);
    apply(&project, &["add", "hex@=0.4.3", "-p", "small-app"]);
    let report = engine_at(&project.dir, T1).verify().expect("verify");
    assert!(!report.matches);
    let differences = describe(&report.differences);
    assert!(differences
        .iter()
        .any(|d| d.starts_with("package only in workspace: hex 0.4.3 ")));
    assert!(
        differences.iter().all(|d| d.contains("only in workspace")),
        "nothing approved is missing: {differences:#?}"
    );
}

#[test]
fn the_approved_crate_on_the_wrong_member_mismatches_and_is_named() {
    let fixture = require_fixture!("small");
    let project = approved(&fixture);
    // Same package list as the approval; a different shape of graph.
    apply(&project, &["add", "either@=1.13.0", "-p", "small-app"]);
    let report = engine_at(&project.dir, T1).verify().expect("verify");
    assert!(
        !report.matches,
        "Phase 0's package-list digest matched here; this one must not"
    );
    let differences = describe(&report.differences);
    println!("{differences:#?}");
    assert!(
        differences.iter().all(|d| !d.starts_with("package ")),
        "the package list is the same"
    );
    assert!(differences.contains(
        &"edge only in workspace: pkg:cargo/small-app@0.1.0?source=path:app -> pkg:cargo/either@1.13.0".to_string()
    ));
    assert!(differences.contains(
        &"edge only in approved: pkg:cargo/small-core@0.1.0?source=path:core -> pkg:cargo/either@1.13.0".to_string()
    ));
    assert!(differences
        .iter()
        .any(|d| d.starts_with("declared only in workspace: app normal - either ")));
    assert!(differences
        .iter()
        .any(|d| d.starts_with("declared only in approved: core normal - either ")));
}

#[test]
fn a_changed_feature_set_mismatches_and_is_named() {
    let fixture = require_fixture!("small");
    let project = project_from(&fixture);
    engine_at(&project.dir, T0).init().expect("init");
    let mut change = ProposedChange::parse("serde@=1.0.210").expect("spec");
    change.member = Some("small-app".to_string());
    engine_at(&project.dir, T1).check(&[change]).expect("check");
    // Applied with a feature the approval did not include.
    apply(
        &project,
        &[
            "add",
            "serde@=1.0.210",
            "-p",
            "small-app",
            "--features",
            "derive",
        ],
    );
    let report = engine_at(&project.dir, T1).verify().expect("verify");
    assert!(!report.matches);
    let differences = describe(&report.differences);
    assert!(differences.iter().any(|d| d
        .contains("declared only in workspace: app normal - serde")
        && d.contains("features=[derive]")));
    assert!(differences.iter().any(|d| d
        .contains("declared only in approved: app normal - serde")
        && d.contains("features=[]")));
}

#[test]
fn a_checkout_in_a_different_directory_matches() {
    let fixture = require_fixture!("small");
    let project = approved(&fixture);
    apply(&project, &["add", "either@=1.13.0", "-p", "small-core"]);

    let elsewhere = tempfile::tempdir().expect("tempdir");
    let moved = elsewhere.path().join("deeper").join("another-checkout");
    copy_tree(&project.dir, &moved);
    copy_tree(&project.dir.join(".depvigil"), &moved.join(".depvigil"));
    let report = engine_at(&moved, T1).verify().expect("verify");
    assert!(report.matches);
    let here = engine_at(&project.dir, T1).verify().expect("verify");
    assert_eq!(report.current_digest, here.current_digest);
}

#[test]
fn verify_from_the_command_line() {
    let fixture = require_fixture!("small");
    let project = approved(&fixture);
    let mismatch = run_cli(&project.dir, None, &["verify"]);
    assert_eq!(mismatch.code, 1, "a mismatch exits non-zero");
    assert!(mismatch.stdout.contains("MISMATCH"), "{}", mismatch.stdout);
    assert!(mismatch.stdout.contains("approved, but missing"));

    apply(&project, &["add", "either@=1.13.0", "-p", "small-core"]);
    let matched = run_cli(&project.dir, None, &["verify", "--json"]);
    assert_eq!(matched.code, 0, "{}", matched.stderr);
    assert_eq!(matched.json()["matches"], true);
    assert_eq!(matched.json()["decision"], "D0001");
}

#[test]
fn verify_needs_an_approved_decision() {
    let fixture = require_fixture!("small");
    let project = project_from(&fixture);
    engine_at(&project.dir, T0).init().expect("init");
    assert!(matches!(
        engine_at(&project.dir, T1).verify(),
        Err(Error::InvalidInput(_))
    ));
}
