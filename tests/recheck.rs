//! `depvigil recheck`: Living Approvals. Covers acceptance item 5 and
//! migrates Phase 0 Test 4, Part B.

mod common;

use std::fs;
use std::path::Path;

use common::{engine_at, project_from, repo_root, run_cli, Project};
use depvigil::advisory::AdvisoryKind;
use depvigil::engine::DecisionStatus;
use depvigil::mirror;
use depvigil::model::ProposedChange;
use depvigil::store::Store;

const T0: &str = "2026-11-27T09:00:00Z";
const T1: &str = "2026-11-27T10:00:00Z";
const T2: &str = "2026-11-27T11:00:00Z";
const T3: &str = "2026-11-27T12:00:00Z";
const T4: &str = "2026-11-27T13:00:00Z";
const T5: &str = "2026-11-27T14:00:00Z";
const T6: &str = "2026-11-27T15:00:00Z";

/// The day the `regex` advisory RUSTSEC-2022-0013 was published.
const CUTOFF: &str = "2022-03-08";
const REGEX_ADVISORY: &str = "RUSTSEC-2022-0013";
const SMALL_APP: &str = "pkg:cargo/small-app@0.1.0?source=path:app";
const VULNERABLE_REGEX: &str = "pkg:cargo/regex@1.5.4";

fn snapshot_file() -> std::path::PathBuf {
    repo_root().join("data/advisories/rustsec-osv.jsonl.gz")
}

fn change(spec: &str, member: &str) -> ProposedChange {
    let mut change = ProposedChange::parse(spec).expect("spec");
    change.member = Some(member.to_string());
    change
}

fn approve(project: &Project, at: &str, change: ProposedChange) -> String {
    let report = engine_at(&project.dir, at).check(&[change]).expect("check");
    assert_eq!(report.candidates[0].verdict, "approved");
    report.candidates[0].decision.clone()
}

/// `small` with only the advisories published before the cut-off loaded,
/// and two approvals: a target that keeps regex 1.5.4 and a control that
/// moves to the fixed regex 1.5.5.
fn scenario(fixture: &Path) -> (Project, String, String) {
    let project = project_from(fixture);
    engine_at(&project.dir, T0).init().expect("init");
    engine_at(&project.dir, T1)
        .load_advisories(&snapshot_file(), Some(CUTOFF))
        .expect("early load");
    let target = approve(&project, T2, change("either@=1.13.0", "small-core"));
    let control = approve(&project, T2, change("regex@=1.5.5", "small-app"));
    (project, target, control)
}

/// Writes one synthetic vulnerability against `regex` below 1.6.0.
fn synthetic_advisory(dir: &Path, withdrawn: Option<&str>) {
    fs::create_dir_all(dir).expect("dir");
    let mut record = serde_json::json!({
        "id": "TEST-2026-0001",
        "published": "2026-01-01T12:00:00Z",
        "modified": "2026-01-01T12:00:00Z",
        "summary": "A synthetic vulnerability",
        "affected": [{
            "package": {"ecosystem": "crates.io", "name": "regex"},
            "database_specific": {"informational": null},
            "ranges": [{"type": "SEMVER", "events": [{"introduced": "0.0.0-0"}, {"fixed": "1.5.5"}]}]
        }]
    });
    if let Some(at) = withdrawn {
        record["withdrawn"] = serde_json::json!(at);
    }
    fs::write(dir.join("TEST-2026-0001.json"), record.to_string()).expect("advisory");
}

#[test]
fn a_later_advisory_invalidates_the_approval_it_affects_and_only_that_one() {
    let fixture = require_fixture!("small");
    let (project, target, control) = scenario(&fixture);
    let mut store = Store::open_read_only(&project.dir).expect("open");
    let target_before = store.raw_decision(&target).expect("raw");
    let control_before = store.raw_decision(&control).expect("raw");
    drop(store);

    // Nothing has changed yet, so nothing is invalidated.
    let quiet = engine_at(&project.dir, T2).recheck().expect("recheck");
    assert_eq!(quiet.approved_decisions, 2);
    assert!(quiet.invalidated.is_empty() && quiet.warnings.is_empty() && quiet.restored.is_empty());

    // The advisory database is updated.
    engine_at(&project.dir, T3)
        .load_advisories(&snapshot_file(), None)
        .expect("full load");
    let report = engine_at(&project.dir, T4).recheck().expect("recheck");

    assert_eq!(
        report.invalidated.len(),
        1,
        "one invalidation per decision and advisory pair"
    );
    let entry = &report.invalidated[0];
    assert_eq!(entry.decision, target);
    assert_eq!(entry.advisory, REGEX_ADVISORY);
    assert_eq!(entry.kind, AdvisoryKind::Vulnerability);
    assert_eq!(entry.purl, VULNERABLE_REGEX);
    assert!(entry.new);
    assert_eq!(
        entry.path.as_deref(),
        Some(&[SMALL_APP.to_string(), VULNERABLE_REGEX.to_string()][..]),
        "the result includes the dependency path"
    );
    // The publication date is still shown; the rule uses first seen.
    assert_eq!(entry.published, "2022-03-08T12:00:00.000Z");
    assert_eq!(
        entry.first_seen_at.as_deref(),
        Some("2026-11-27T12:00:00.000Z")
    );

    assert!(
        !report.invalidated.iter().any(|e| e.decision == control),
        "the control decision is untouched"
    );

    let mut store = Store::open_read_only(&project.dir).expect("open");
    assert_eq!(
        store.raw_decision(&target).expect("raw"),
        target_before,
        "Decision nodes are never modified"
    );
    assert_eq!(store.raw_decision(&control).expect("raw"), control_before);
    let counts = store.counts().expect("counts");
    assert_eq!(counts["node:Invalidation"], 1);
    assert_eq!(counts["relationship:INVALIDATES"], 1);
    assert_eq!(counts["relationship:CAUSED_BY"], 1);
    let decisions = store.decisions().expect("decisions");
    assert_eq!(
        DecisionStatus::of(&decisions[0]),
        DecisionStatus::Invalidated
    );
    assert_eq!(DecisionStatus::of(&decisions[1]), DecisionStatus::Valid);
    assert_eq!(
        decisions[0].load.as_deref(),
        Some("load-0001"),
        "the approval states the evidence it had"
    );
    drop(store);

    // A second run creates nothing new.
    let lines_before = mirror::read(&project.dir).expect("mirror").len();
    let again = engine_at(&project.dir, T5)
        .recheck()
        .expect("second recheck");
    assert_eq!(again.invalidated.len(), 1);
    assert!(
        !again.invalidated[0].new,
        "the invalidation is standing, not new"
    );
    let mut store = Store::open_read_only(&project.dir).expect("open");
    assert_eq!(
        store.counts().expect("counts"),
        counts,
        "a second run creates nothing"
    );
    assert_eq!(
        mirror::read(&project.dir).expect("mirror").len(),
        lines_before
    );

    // The text log carries the invalidation.
    let lines = mirror::read(&project.dir).expect("mirror");
    let logged: Vec<_> = lines
        .iter()
        .filter(|l| l["type"] == "invalidation")
        .collect();
    assert_eq!(logged.len(), 1);
    assert_eq!(logged[0]["decision"], target.as_str());
    assert_eq!(logged[0]["advisory"], REGEX_ADVISORY);
    assert_eq!(
        logged[0]["id"],
        store.invalidations().expect("invalidations")[0].id.as_str()
    );
}

#[test]
fn a_second_advisory_gets_its_own_invalidation() {
    let fixture = require_fixture!("small");
    let (project, target, _control) = scenario(&fixture);
    engine_at(&project.dir, T3)
        .load_advisories(&snapshot_file(), None)
        .expect("full load");
    engine_at(&project.dir, T4).recheck().expect("recheck");

    let data = tempfile::tempdir().expect("tempdir");
    synthetic_advisory(data.path(), None);
    engine_at(&project.dir, T5)
        .load_advisories(data.path(), None)
        .expect("synthetic load");
    let report = engine_at(&project.dir, T6).recheck().expect("recheck");

    let for_target: Vec<_> = report
        .invalidated
        .iter()
        .filter(|e| e.decision == target)
        .collect();
    assert_eq!(for_target.len(), 2, "one per decision and advisory pair");
    let new: Vec<&str> = for_target
        .iter()
        .filter(|e| e.new)
        .map(|e| e.advisory.as_str())
        .collect();
    assert_eq!(new, ["TEST-2026-0001"], "only the second advisory is new");

    let mut store = Store::open_read_only(&project.dir).expect("open");
    let ids: Vec<String> = store
        .invalidations()
        .expect("invalidations")
        .into_iter()
        .map(|i| i.id)
        .collect();
    assert_eq!(
        ids,
        [
            format!("INV-{target}-{REGEX_ADVISORY}"),
            format!("INV-{target}-TEST-2026-0001")
        ]
    );
}

#[test]
fn a_notice_that_arrives_later_warns_and_invalidates_nothing() {
    let fixture = require_fixture!("small");
    let project = project_from(&fixture);
    engine_at(&project.dir, T0).init().expect("init");
    engine_at(&project.dir, T1)
        .load_advisories(&snapshot_file(), Some(CUTOFF))
        .expect("early load");
    // atty already carries one notice here; a second one is published later.
    let report = engine_at(&project.dir, T2)
        .check(&[change("atty@=0.2.14", "small-app")])
        .expect("check");
    let decision = report.candidates[0].decision.clone();
    assert_eq!(report.candidates[0].verdict, "approved");

    engine_at(&project.dir, T3)
        .load_advisories(&snapshot_file(), None)
        .expect("full load");
    let report = engine_at(&project.dir, T4).recheck().expect("recheck");

    // atty's "unmaintained" notice (RUSTSEC-2024-0375) arrived after the approval.
    let notice = report
        .warnings
        .iter()
        .find(|w| w.decision == decision && w.advisory == "RUSTSEC-2024-0375")
        .expect("the notice is reported as a warning");
    assert_eq!(notice.kind, AdvisoryKind::Notice);
    assert!(notice.path.is_some());
    assert!(notice
        .note
        .as_deref()
        .unwrap()
        .contains("does not invalidate"));
    assert!(
        !report
            .invalidated
            .iter()
            .any(|e| e.advisory == "RUSTSEC-2024-0375"),
        "a notice never invalidates"
    );
    let mut store = Store::open_read_only(&project.dir).expect("open");
    assert!(
        store
            .invalidations()
            .expect("invalidations")
            .iter()
            .all(|i| i.advisory != "RUSTSEC-2024-0375"),
        "no Invalidation node is written for a notice"
    );
}

#[test]
fn a_withdrawn_advisory_restores_the_decision() {
    let fixture = require_fixture!("small");
    let project = project_from(&fixture);
    engine_at(&project.dir, T0).init().expect("init");
    let decision = approve(&project, T1, change("either@=1.13.0", "small-core"));
    let mut store = Store::open_read_only(&project.dir).expect("open");
    let before = store.raw_decision(&decision).expect("raw");
    drop(store);

    let data = tempfile::tempdir().expect("tempdir");
    synthetic_advisory(data.path(), None);
    engine_at(&project.dir, T2)
        .load_advisories(data.path(), None)
        .expect("load");
    let invalidated = engine_at(&project.dir, T3).recheck().expect("recheck");
    assert_eq!(invalidated.invalidated.len(), 1);
    assert!(invalidated.restored.is_empty());

    // The advisory is withdrawn upstream and the data is loaded again.
    synthetic_advisory(data.path(), Some("2026-06-01T00:00:00Z"));
    engine_at(&project.dir, T4)
        .load_advisories(data.path(), None)
        .expect("reload");
    let report = engine_at(&project.dir, T5).recheck().expect("recheck");

    assert!(
        report.invalidated.is_empty(),
        "a withdrawn advisory invalidates nothing"
    );
    assert_eq!(report.restored.len(), 1);
    let restored = &report.restored[0];
    assert_eq!(restored.decision, decision);
    assert_eq!(restored.advisory, "TEST-2026-0001");
    assert_eq!(
        restored.path.as_deref(),
        Some(&[SMALL_APP.to_string(), VULNERABLE_REGEX.to_string()][..])
    );
    assert!(restored.note.as_deref().unwrap().contains("withdrawn"));

    let log = engine_at(&project.dir, T5).log().expect("log");
    assert_eq!(log[0].status, DecisionStatus::Restored);

    let mut store = Store::open_read_only(&project.dir).expect("open");
    assert_eq!(
        store.raw_decision(&decision).expect("raw"),
        before,
        "the Decision node is never modified"
    );
    assert_eq!(
        store.counts().expect("counts")["node:Invalidation"],
        1,
        "history is kept"
    );
}

#[test]
fn recheck_from_the_command_line() {
    let fixture = require_fixture!("small");
    let (project, target, _control) = scenario(&fixture);
    let quiet = run_cli(&project.dir, Some(T2), &["recheck"]);
    assert_eq!(quiet.code, 0);
    assert!(
        quiet.stdout.contains("Every approval still holds"),
        "{}",
        quiet.stdout
    );

    let snapshot = snapshot_file();
    assert_eq!(
        run_cli(
            &project.dir,
            Some(T3),
            &["advisories", "load", snapshot.to_str().unwrap()]
        )
        .code,
        0
    );
    let text = run_cli(&project.dir, Some(T4), &["recheck"]);
    assert_eq!(text.code, 1, "an invalidated approval exits non-zero");
    assert!(
        text.stdout.contains("INVALIDATED (1 new, 0 standing)"),
        "{}",
        text.stdout
    );
    assert!(text.stdout.contains("path: small-app@0.1.0 -> regex@1.5.4"));

    let json = run_cli(&project.dir, Some(T5), &["recheck", "--json"]);
    assert_eq!(json.code, 1);
    let report = json.json();
    assert_eq!(report["invalidated"][0]["decision"], target.as_str());
    assert_eq!(report["invalidated"][0]["new"], false);
}
