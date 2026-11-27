//! `depvigil log` and `depvigil why`. Covers acceptance item 6: the output
//! matches the store on a scripted sequence of decisions.

mod common;

use common::{cargo_ground_truth, engine_at, project_from, repo_root, run_cli};
use depvigil::engine::DecisionStatus;
use depvigil::model::ProposedChange;
use depvigil::store::Store;

const SMALL_APP: &str = "pkg:cargo/small-app@0.1.0?source=path:app";
const SMALL_CORE: &str = "pkg:cargo/small-core@0.1.0?source=path:core";

fn change(spec: &str, member: &str) -> ProposedChange {
    let mut change = ProposedChange::parse(spec).expect("spec");
    change.member = Some(member.to_string());
    change
}

#[test]
fn log_and_why_match_the_store_on_a_scripted_sequence() {
    let fixture = require_fixture!("small");
    let project = project_from(&fixture);
    let snapshot = repo_root().join("data/advisories/rustsec-osv.jsonl.gz");
    let at = |hour: u32| format!("2026-11-27T{hour:02}:00:00Z");

    // The script: an approval, a rejection, an approval with warnings, then
    // new evidence that invalidates the first approval.
    engine_at(&project.dir, &at(9)).init().expect("init");
    engine_at(&project.dir, &at(10))
        .load_advisories(&snapshot, Some("2022-03-08"))
        .expect("early load");
    engine_at(&project.dir, &at(11))
        .check(&[change("either@=1.13.0", "small-core")])
        .expect("check 1");
    engine_at(&project.dir, &at(12))
        .check(&[change("chrono@=0.4.19", "small-app")])
        .expect("check 2");
    engine_at(&project.dir, &at(13))
        .check(&[change("regex@=1.5.5", "small-app")])
        .expect("check 3");
    engine_at(&project.dir, &at(14))
        .load_advisories(&snapshot, None)
        .expect("full load");
    engine_at(&project.dir, &at(15)).recheck().expect("recheck");

    // log, against what was scripted.
    let log = engine_at(&project.dir, &at(16)).log().expect("log");
    let summary: Vec<(&str, &str, &str, DecisionStatus, &str)> = log
        .iter()
        .map(|e| {
            (
                e.id.as_str(),
                e.verdict.as_str(),
                e.decided_at.as_str(),
                e.status,
                e.change.as_str(),
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            (
                "D0001",
                "approved",
                "2026-11-27T11:00:00.000Z",
                DecisionStatus::Invalidated,
                "add either@=1.13.0 to small-core"
            ),
            (
                "D0002",
                "rejected",
                "2026-11-27T12:00:00.000Z",
                DecisionStatus::Rejected,
                "add chrono@=0.4.19 to small-app"
            ),
            (
                "D0003",
                "approved",
                "2026-11-27T13:00:00.000Z",
                DecisionStatus::Valid,
                "add regex@=1.5.5 to small-app"
            ),
        ]
    );
    assert_eq!(log[0].invalidated_by, ["RUSTSEC-2022-0013"]);
    assert!(log[1].invalidated_by.is_empty() && log[2].invalidated_by.is_empty());
    assert_eq!(log[0].precision, "lockfile");
    assert_eq!(log[1].precision, "full");
    assert!(log
        .iter()
        .all(|e| e.advisory_load.as_deref() == Some("load-0001")));

    // log, against the store itself.
    let mut store = Store::open_read_only(&project.dir).expect("open");
    let stored = store.decisions().expect("decisions");
    assert_eq!(log.len(), stored.len());
    for (entry, decision) in log.iter().zip(&stored) {
        assert_eq!(entry.id, decision.id);
        assert_eq!(entry.verdict, decision.verdict);
        assert_eq!(entry.decided_at, decision.decided_at);
        assert_eq!(entry.digest, decision.digest);
        assert_eq!(entry.status, DecisionStatus::of(decision));
    }
    drop(store);

    // why: a direct dependency of one member.
    let regex = engine_at(&project.dir, &at(16))
        .why("regex")
        .expect("why regex");
    assert_eq!(regex.versions.len(), 1);
    assert_eq!(regex.versions[0].purl, "pkg:cargo/regex@1.5.4");
    assert_eq!(regex.versions[0].active, Some(true));
    assert_eq!(
        regex.versions[0].path.as_deref(),
        Some(&[SMALL_APP.to_string(), "pkg:cargo/regex@1.5.4".to_string()][..])
    );
    // D0001 was invalidated through regex; D0003 changed its version.
    let mentioned: Vec<&str> = regex.decisions.iter().map(|d| d.id.as_str()).collect();
    assert_eq!(mentioned, ["D0001", "D0003"]);

    // why: a transitive dependency, two hops from a member.
    let memchr = engine_at(&project.dir, &at(16))
        .why("memchr")
        .expect("why memchr");
    let path = memchr.versions[0].path.as_ref().expect("a path");
    assert_eq!(path.first().map(String::as_str), Some(SMALL_APP));
    assert_eq!(path.last(), Some(&memchr.versions[0].purl));
    assert!(path.len() >= 3, "{path:?}");
    assert!(memchr.decisions.is_empty());

    // why: a path dependency reached through the other member.
    let marker = engine_at(&project.dir, &at(16))
        .why("marker")
        .expect("why marker");
    assert_eq!(
        marker.versions[0].path.as_deref(),
        Some(
            &[
                SMALL_CORE.to_string(),
                "pkg:cargo/marker@0.1.0?source=path:vendor/marker".to_string()
            ][..]
        )
    );

    // why: a crate that was only ever proposed.
    let chrono = engine_at(&project.dir, &at(16))
        .why("chrono")
        .expect("why chrono");
    assert!(chrono.versions.is_empty(), "chrono is not in the baseline");
    assert_eq!(chrono.decisions.len(), 1);
    assert_eq!(chrono.decisions[0].id, "D0002");
    assert_eq!(chrono.decisions[0].status, DecisionStatus::Rejected);

    let unknown = engine_at(&project.dir, &at(16))
        .why("no-such-crate")
        .expect("why unknown");
    assert!(unknown.versions.is_empty() && unknown.decisions.is_empty());

    // The command line prints the same.
    let text = run_cli(&project.dir, None, &["log"]);
    assert_eq!(text.code, 0);
    let lines: Vec<&str> = text.stdout.lines().collect();
    assert_eq!(lines.len(), 3);
    assert!(
        lines[0].starts_with(
            "D0001  2026-11-27T11:00:00.000Z  approved  invalidated by RUSTSEC-2022-0013"
        ),
        "{}",
        lines[0]
    );
    assert!(
        lines[1].starts_with("D0002  2026-11-27T12:00:00.000Z  rejected"),
        "{}",
        lines[1]
    );
    let json = run_cli(&project.dir, None, &["log", "--json"]).json();
    assert_eq!(json.as_array().map(Vec::len), Some(3));
    assert_eq!(json[0]["status"], "invalidated");

    let why = run_cli(&project.dir, None, &["why", "regex", "--json"]).json();
    assert_eq!(why["versions"][0]["purl"], "pkg:cargo/regex@1.5.4");
    assert_eq!(why["decisions"].as_array().map(Vec::len), Some(2));
    let why_text = run_cli(&project.dir, None, &["why", "regex"]);
    assert!(
        why_text
            .stdout
            .contains("path: small-app@0.1.0 -> regex@1.5.4"),
        "{}",
        why_text.stdout
    );
}

#[test]
fn why_follows_a_new_baseline() {
    let fixture = require_fixture!("small");
    let project = project_from(&fixture);
    engine_at(&project.dir, "2026-11-27T09:00:00Z")
        .init()
        .expect("init");
    assert!(engine_at(&project.dir, "2026-11-27T09:00:00Z")
        .why("either")
        .expect("why")
        .versions
        .is_empty());

    cargo_ground_truth(&project.dir, &["add", "either@=1.13.0", "-p", "small-core"]);
    let report = engine_at(&project.dir, "2026-11-27T10:00:00Z")
        .init()
        .expect("second init");
    assert!(report.baseline_changed);
    let why = engine_at(&project.dir, "2026-11-27T10:00:00Z")
        .why("either")
        .expect("why");
    assert_eq!(
        why.versions[0].path.as_deref(),
        Some(
            &[
                SMALL_CORE.to_string(),
                "pkg:cargo/either@1.13.0".to_string()
            ][..]
        )
    );
}
