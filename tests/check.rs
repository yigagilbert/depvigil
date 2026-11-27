//! `depvigil check`: the two-step simulation, the policy and the recorded
//! decisions. Covers acceptance item 3 and migrates the snapshot diff of
//! Phase 0 Test 2.

mod common;

use std::fs;
use std::path::Path;

use common::{
    build_evidence, cargo_ground_truth, engine_at, project_from, repo_root, run_cli, tree_hashes,
    Project,
};
use depvigil::advisory::AdvisoryKind;
use depvigil::ecosystem::Ecosystem;
use depvigil::mirror;
use depvigil::model::ProposedChange;
use depvigil::store::Store;
use depvigil::Error;

const T0: &str = "2026-11-27T09:00:00Z";
const T1: &str = "2026-11-27T10:00:00Z";
const T2: &str = "2026-11-27T11:00:00Z";

const SMALL_APP: &str = "pkg:cargo/small-app@0.1.0?source=path:app";

/// A copy of `small`, initialised, with every advisory loaded.
fn prepared(fixture: &Path) -> Project {
    let project = project_from(fixture);
    engine_at(&project.dir, T0).init().expect("init");
    engine_at(&project.dir, T1)
        .load_advisories(
            &repo_root().join("data/advisories/rustsec-osv.jsonl.gz"),
            None,
        )
        .expect("load advisories");
    project
}

fn change(spec: &str, member: &str) -> ProposedChange {
    let mut change = ProposedChange::parse(spec).expect("valid spec");
    change.member = Some(member.to_string());
    change
}

#[test]
fn a_clean_candidate_is_approved_without_running_cargo_metadata() {
    let fixture = require_fixture!("small");
    let project = prepared(&fixture);
    let before = tree_hashes(&project.dir);
    let engine = engine_at(&project.dir, T2);

    let report = engine
        .check(&[change("either@=1.13.0", "small-core")])
        .expect("check");
    let candidate = &report.candidates[0];
    assert_eq!(candidate.verdict, "approved");
    assert!(!candidate.precise_step, "the lockfile-only step decided");
    assert!(candidate.rejections.is_empty() && candidate.warnings.is_empty());
    assert_eq!(candidate.diff.added, ["pkg:cargo/either@1.13.0"]);
    assert_eq!(
        candidate.existing.len(),
        3,
        "baseline findings are context, not grounds"
    );
    assert_eq!(report.best.as_deref(), Some("either@=1.13.0"));
    assert_eq!(report.advisory_load.as_deref(), Some("load-0001"));

    // Proof that cargo metadata was not invoked: the engine's own record of
    // every Cargo command it ran.
    let commands = engine.ecosystem().tool_invocations();
    assert_eq!(commands, ["cargo add either@=1.13.0 --package small-core"]);
    assert_eq!(report.commands, commands);

    let mut store = Store::open_read_only(&project.dir).expect("open");
    let decisions = store.decisions().expect("decisions");
    assert_eq!(decisions.len(), 1);
    assert_eq!(decisions[0].id, "D0001");
    assert_eq!(decisions[0].verdict, "approved");
    assert_eq!(decisions[0].precision, "lockfile");
    assert_eq!(decisions[0].digest, candidate.digest);
    assert_eq!(
        decisions[0].load.as_deref(),
        Some("load-0001"),
        "the approval states its evidence"
    );
    assert_eq!(decisions[0].decided_at, "2026-11-27T11:00:00.000Z");

    assert_eq!(
        tree_hashes(&project.dir),
        before,
        "the workspace is byte-for-byte unchanged"
    );
    assert!(
        build_evidence(&project.dir).is_empty(),
        "nothing compiled, no build script ran"
    );
}

#[test]
fn a_candidate_that_introduces_a_vulnerability_is_rejected_with_its_path() {
    let fixture = require_fixture!("small");
    let project = prepared(&fixture);
    let before = tree_hashes(&project.dir);
    let engine = engine_at(&project.dir, T2);

    let report = engine
        .check(&[change("chrono@=0.4.19", "small-app")])
        .expect("check");
    let candidate = &report.candidates[0];
    assert_eq!(candidate.verdict, "rejected");
    assert!(
        candidate.precise_step,
        "a policy hit triggers the precise step"
    );
    assert!(report
        .commands
        .iter()
        .any(|c| c.starts_with("cargo metadata")));
    assert_eq!(report.best, None);

    assert_eq!(candidate.rejections.len(), 1);
    let rejection = &candidate.rejections[0];
    assert_eq!(rejection.advisory, "RUSTSEC-2020-0159");
    assert_eq!(rejection.kind, AdvisoryKind::Vulnerability);
    assert_eq!(rejection.purl, "pkg:cargo/chrono@0.4.19");
    assert_eq!(rejection.active, Some(true));
    assert_eq!(
        rejection.path.as_deref(),
        Some(&[SMALL_APP.to_string(), "pkg:cargo/chrono@0.4.19".to_string()][..]),
        "a dependency path from a workspace member to the affected version"
    );
    // chrono is also flagged unmaintained: a notice warns, it does not reject.
    assert!(candidate
        .warnings
        .iter()
        .all(|w| w.kind == AdvisoryKind::Notice));

    let mut store = Store::open_read_only(&project.dir).expect("open");
    let decisions = store.decisions().expect("decisions");
    assert_eq!(decisions[0].verdict, "rejected");
    assert_eq!(decisions[0].precision, "full");
    let counts = store.counts().expect("counts");
    assert_eq!(counts["relationship:REJECTS"], 1);
    assert!(counts["relationship:EVIDENCED_BY"] >= 1);

    assert_eq!(
        tree_hashes(&project.dir),
        before,
        "the workspace is byte-for-byte unchanged"
    );
    assert!(
        build_evidence(&project.dir).is_empty(),
        "nothing compiled, no build script ran"
    );
}

#[test]
fn a_hit_on_an_inactive_package_runs_the_precise_step_and_only_warns() {
    let fixture = require_fixture!("small");
    let project = prepared(&fixture);
    let engine = engine_at(&project.dir, T2);

    // An optional dependency is locked but stays out of the resolve graph.
    let mut optional = change("chrono@=0.4.19", "small-app");
    optional.optional = true;
    let report = engine.check(&[optional]).expect("check");
    let candidate = &report.candidates[0];
    assert!(
        candidate.precise_step,
        "the lockfile hit triggers the precise step"
    );
    assert_eq!(
        candidate.verdict, "approved",
        "an inactive hit is a warning, not a rejection"
    );
    assert!(candidate.rejections.is_empty());
    let warning = candidate
        .warnings
        .iter()
        .find(|w| w.advisory == "RUSTSEC-2020-0159")
        .expect("the vulnerability is reported as a warning");
    assert_eq!(warning.kind, AdvisoryKind::Vulnerability);
    assert_eq!(warning.active, Some(false));
    assert!(warning.path.is_some());
}

#[test]
fn a_notice_warns_and_does_not_reject() {
    let fixture = require_fixture!("small");
    let project = prepared(&fixture);
    let report = engine_at(&project.dir, T2)
        .check(&[change("atty@=0.2.14", "small-app")])
        .expect("check");
    let candidate = &report.candidates[0];
    assert_eq!(candidate.verdict, "approved");
    assert!(candidate.precise_step);
    assert_eq!(candidate.warnings.len(), 2);
    assert!(candidate
        .warnings
        .iter()
        .all(|w| w.kind == AdvisoryKind::Notice && w.path.is_some()));
}

#[test]
fn several_candidates_get_a_verdict_each_and_the_best_is_marked() {
    let fixture = require_fixture!("small");
    let project = prepared(&fixture);
    let engine = engine_at(&project.dir, T2);
    let report = engine
        .check(&[
            change("either@=1.12.0", "small-core"),
            change("chrono@=0.4.19", "small-core"),
            change("either@=1.13.0", "small-core"),
        ])
        .expect("check");

    let verdicts: Vec<(&str, &str, bool)> = report
        .candidates
        .iter()
        .map(|c| (c.spec.as_str(), c.verdict.as_str(), c.best))
        .collect();
    assert_eq!(
        verdicts,
        [
            ("either@=1.12.0", "approved", false),
            ("chrono@=0.4.19", "rejected", false),
            ("either@=1.13.0", "approved", true),
        ]
    );
    assert_eq!(report.best.as_deref(), Some("either@=1.13.0"));
    let reason = report.best_reason.as_deref().expect("a reason");
    assert!(reason.contains("highest version (1.13.0)"), "{reason}");

    // The best candidate is the latest approved decision, the one `verify`
    // compares against.
    let mut store = Store::open_read_only(&project.dir).expect("open");
    let decisions = store.decisions().expect("decisions");
    assert_eq!(decisions.len(), 3);
    let latest_approved = decisions
        .iter()
        .rev()
        .find(|d| d.verdict == "approved")
        .expect("approved");
    assert_eq!(latest_approved.digest, report.candidates[2].digest);

    // Warnings rank below a clean candidate.
    let report = engine
        .check(&[
            change("atty@=0.2.14", "small-app"),
            change("either@=1.13.0", "small-app"),
        ])
        .expect("check");
    assert_eq!(report.best.as_deref(), Some("either@=1.13.0"));
    assert_eq!(report.best_reason.as_deref(), Some("it has no warnings"));
}

#[test]
fn more_than_four_candidates_are_refused() {
    let fixture = require_fixture!("small");
    let project = prepared(&fixture);
    let five: Vec<ProposedChange> = (0..5).map(|_| change("either", "small-core")).collect();
    assert!(matches!(
        engine_at(&project.dir, T2).check(&five),
        Err(Error::InvalidInput(_))
    ));
    assert!(matches!(
        engine_at(&project.dir, T2).check(&[]),
        Err(Error::InvalidInput(_))
    ));
}

#[test]
fn removing_a_baseline_vulnerability_is_reported_as_an_improvement() {
    let fixture = require_fixture!("small");
    let project = prepared(&fixture);
    let report = engine_at(&project.dir, T2)
        .check(&[change("smallvec@=1.6.1", "small-app")])
        .expect("check");
    let candidate = &report.candidates[0];
    assert_eq!(candidate.verdict, "approved");
    assert!(!candidate.precise_step);
    assert_eq!(candidate.improvements.len(), 1);
    assert_eq!(candidate.improvements[0].advisory, "RUSTSEC-2021-0003");
    assert_eq!(candidate.existing.len(), 2);
    // The Cypher diff of Phase 0 Test 2.
    assert_eq!(candidate.diff.added, ["pkg:cargo/smallvec@1.6.1"]);
    assert_eq!(candidate.diff.removed, ["pkg:cargo/smallvec@1.6.0"]);
    assert_eq!(
        candidate.diff.changed,
        ["pkg:cargo/smallvec@1.6.0 -> pkg:cargo/smallvec@1.6.1"]
    );
}

#[test]
fn strict_mode_rejects_on_any_vulnerability() {
    let fixture = require_fixture!("small");
    let project = prepared(&fixture);
    fs::write(
        project.dir.join("depvigil.toml"),
        "[policy]\nmode = \"strict\"\n",
    )
    .expect("policy");
    let report = engine_at(&project.dir, T2)
        .check(&[change("either@=1.13.0", "small-core")])
        .expect("check");
    let candidate = &report.candidates[0];
    assert_eq!(candidate.verdict, "rejected");
    assert_eq!(
        candidate.rejections.len(),
        3,
        "the three baseline vulnerabilities count in strict mode"
    );
    assert!(candidate.rejections.iter().all(|r| r.path.is_some()));

    fs::write(
        project.dir.join("depvigil.toml"),
        "[policy]\nmode = \"lenient\"\n",
    )
    .expect("policy");
    assert!(matches!(
        engine_at(&project.dir, T2).check(&[change("either@=1.13.0", "small-core")]),
        Err(Error::Parse { .. })
    ));
}

#[test]
fn with_no_advisories_loaded_a_candidate_is_approved_and_says_so() {
    let fixture = require_fixture!("small");
    let project = project_from(&fixture);
    engine_at(&project.dir, T0).init().expect("init");
    let report = engine_at(&project.dir, T2)
        .check(&[change("either@=1.13.0", "small-core")])
        .expect("check");
    assert_eq!(report.candidates[0].verdict, "approved");
    assert_eq!(report.advisory_load, None);
}

#[test]
fn a_changed_workspace_needs_a_new_baseline() {
    let fixture = require_fixture!("small");
    let project = prepared(&fixture);
    cargo_ground_truth(&project.dir, &["add", "either@=1.13.0", "-p", "small-core"]);
    assert!(matches!(
        engine_at(&project.dir, T2).check(&[change("itoa", "small-app")]),
        Err(Error::StaleBaseline)
    ));
}

#[test]
fn the_text_log_agrees_with_the_store() {
    let fixture = require_fixture!("small");
    let project = prepared(&fixture);
    let engine = engine_at(&project.dir, T2);
    engine
        .check(&[change("either@=1.13.0", "small-core")])
        .expect("check");
    engine
        .check(&[
            change("chrono@=0.4.19", "small-app"),
            change("atty@=0.2.14", "small-app"),
        ])
        .expect("check");

    let mut store = Store::open_read_only(&project.dir).expect("open");
    let decisions = store.decisions().expect("decisions");
    let lines = mirror::read(&project.dir).expect("mirror");
    assert_eq!(lines.len(), decisions.len(), "one line per decision");
    assert_eq!(decisions.len(), 3);
    for (line, decision) in lines.iter().zip(&decisions) {
        assert_eq!(line["type"], "decision");
        assert_eq!(line["id"], decision.id);
        assert_eq!(line["verdict"], decision.verdict);
        assert_eq!(line["decided_at"], decision.decided_at);
        assert_eq!(line["digest"], decision.digest);
        assert_eq!(line["change"], decision.change);
        assert_eq!(line["candidate"], decision.candidate);
        assert_eq!(line["baseline"], decision.baseline);
        assert_eq!(line["precision"], decision.precision);
        assert_eq!(line["advisory_load"], serde_json::json!(decision.load));
    }
    let text = fs::read_to_string(mirror::path_in(&project.dir)).expect("read mirror");
    assert!(
        !text.contains(project.dir.to_str().unwrap()),
        "no absolute path in the text log"
    );
}

#[test]
fn check_from_the_command_line() {
    let fixture = require_fixture!("small");
    let project = prepared(&fixture);

    let approved = run_cli(
        &project.dir,
        Some(T2),
        &["check", "either@=1.13.0", "-p", "small-core", "--json"],
    );
    assert_eq!(approved.code, 0, "{}", approved.stderr);
    let report = approved.json();
    assert_eq!(report["candidates"][0]["verdict"], "approved");
    assert_eq!(report["candidates"][0]["precise_step"], false);
    assert_eq!(
        report["commands"],
        serde_json::json!(["cargo add either@=1.13.0 --package small-core"])
    );

    let rejected = run_cli(
        &project.dir,
        Some(T2),
        &["check", "chrono@=0.4.19", "-p", "small-app"],
    );
    assert_eq!(rejected.code, 1, "a rejection exits non-zero");
    assert!(
        rejected.stdout.contains("REJECTED chrono@=0.4.19"),
        "{}",
        rejected.stdout
    );
    assert!(rejected.stdout.contains("RUSTSEC-2020-0159"));
    assert!(rejected
        .stdout
        .contains("path: small-app@0.1.0 -> chrono@0.4.19"));

    let both = run_cli(
        &project.dir,
        Some(T2),
        &[
            "check",
            "either@=1.12.0",
            "either@=1.13.0",
            "-p",
            "small-core",
        ],
    );
    assert_eq!(both.code, 0);
    assert!(
        both.stdout
            .contains("Best acceptable candidate: either@=1.13.0"),
        "{}",
        both.stdout
    );

    let offline = run_cli(
        &project.dir,
        Some(T2),
        &[
            "check",
            "either@=1.13.0",
            "-p",
            "small-core",
            "--offline",
            "--json",
        ],
    );
    assert_eq!(
        offline.code, 0,
        "the warm cache allows an offline check: {}",
        offline.stderr
    );
    assert_eq!(
        offline.json()["commands"],
        serde_json::json!(["cargo add either@=1.13.0 --package small-core --offline"])
    );
}

#[test]
fn unsupported_workspaces_exit_with_their_own_code() {
    let cases = [
        (
            "[dependencies]\nleft = { git = \"https://example.invalid/left.git\" }\n",
            "git dependency `left`",
        ),
        (
            "[patch.crates-io]\nserde = { path = \"vendor/serde\" }\n",
            "[patch] section",
        ),
        (
            "[dependencies]\nsibling = { path = \"../sibling\" }\n",
            "points outside the workspace root",
        ),
    ];
    for (extra, message) in cases {
        let temp = tempfile::tempdir().expect("tempdir");
        let manifest = format!(
            "[package]\nname = \"tiny\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n{extra}"
        );
        fs::write(temp.path().join("Cargo.toml"), manifest).expect("manifest");
        fs::write(
            temp.path().join("Cargo.lock"),
            "version = 4\n\n[[package]]\nname = \"tiny\"\nversion = \"0.1.0\"\n",
        )
        .expect("lockfile");
        for args in [&["init"][..], &["check", "either"][..]] {
            let output = run_cli(temp.path(), None, args);
            assert_eq!(output.code, 3, "{args:?}: {}", output.stderr);
            assert!(output.stderr.contains(message), "{}", output.stderr);
        }
        let json = run_cli(temp.path(), None, &["init", "--json"]);
        assert_eq!(json.code, 3);
        assert_eq!(json.json()["error"]["exit_code"], 3);
        assert!(json.json()["error"]["unsupported"].is_array());
        assert!(
            !temp.path().join(".depvigil").exists(),
            "nothing is written for an unsupported workspace"
        );
    }
}
