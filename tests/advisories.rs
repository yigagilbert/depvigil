//! `depvigil advisories load`: kinds, `AFFECTS` relationships and agreement
//! with `cargo audit`.
//!
//! Covers acceptance item 2 and migrates Phase 0 Test 4, Part A.

mod common;

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use common::{engine_at, fixture, project_from, repo_root, run_cli};
use depvigil::advisory::AdvisoryKind;
use depvigil::store::Store;

const T0: &str = "2026-11-27T09:00:00Z";
const T1: &str = "2026-11-27T10:00:00Z";
const T2: &str = "2026-11-27T11:00:00Z";

/// `advisory package version`.
type Findings = BTreeSet<String>;

fn advisories_dir() -> PathBuf {
    repo_root().join("data").join("advisories").join("osv")
}

/// What the store says about the baseline: live vulnerabilities and notices.
fn store_findings(project: &Path) -> (Findings, Findings) {
    let mut store = Store::open_read_only(project).expect("open store");
    let baseline = store.baseline().expect("baseline").expect("has baseline");
    let (mut vulnerabilities, mut notices) = (Findings::new(), Findings::new());
    for finding in store.findings(&baseline.id).expect("findings") {
        if finding.advisory.withdrawn.is_some() {
            continue;
        }
        let line = format!(
            "{} {} {}",
            finding.advisory.id, finding.name, finding.version
        );
        match finding.advisory.kind {
            AdvisoryKind::Vulnerability => vulnerabilities.insert(line),
            AdvisoryKind::Notice => notices.insert(line),
        };
    }
    (vulnerabilities, notices)
}

/// The recorded `cargo audit` result for a fixture.
fn recorded(name: &str) -> (Findings, Findings) {
    let path = repo_root()
        .join("tests/golden")
        .join(format!("cargo-audit-{name}.json"));
    let json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(path).expect("golden file")).expect("JSON");
    let set = |key: &str| {
        json[key]
            .as_array()
            .expect("array")
            .iter()
            .map(|f| {
                format!(
                    "{} {} {}",
                    f["advisory"].as_str().unwrap(),
                    f["package"].as_str().unwrap(),
                    f["version"].as_str().unwrap()
                )
            })
            .collect()
    };
    (set("vulnerabilities"), set("notices"))
}

/// `cargo audit` run now, when the advisory-db submodule and the tool are
/// both present.
fn live_cargo_audit(lockfile: &Path) -> Option<(Findings, Findings)> {
    let db = fixture("advisory-db")?;
    let output = Command::new("cargo")
        .args(["audit", "--json", "--no-fetch", "--db"])
        .arg(db)
        .arg("--file")
        .arg(lockfile)
        .output()
        .ok()?;
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    let line = |item: &serde_json::Value| {
        item["advisory"]["id"].as_str().map(|id| {
            format!(
                "{id} {} {}",
                item["package"]["name"].as_str().unwrap_or_default(),
                item["package"]["version"].as_str().unwrap_or_default()
            )
        })
    };
    let vulnerabilities = report["vulnerabilities"]["list"]
        .as_array()?
        .iter()
        .filter_map(line)
        .collect();
    let notices = report["warnings"]
        .as_object()?
        .values()
        .flat_map(|list| list.as_array().into_iter().flatten())
        .filter_map(line)
        .collect();
    Some((vulnerabilities, notices))
}

fn assert_equals_cargo_audit(
    fixture_name: &str,
    fixture_dir: &Path,
    expected_counts: (usize, usize),
) {
    let project = project_from(fixture_dir);
    engine_at(&project.dir, T0).init().expect("init");
    let report = engine_at(&project.dir, T1)
        .load_advisories(&advisories_dir(), None)
        .expect("load advisories");
    println!(
        "{fixture_name}: loaded {} advisories ({} vulnerabilities, {} notices, {} withdrawn), {} AFFECTS created",
        report.load.advisory_count, report.vulnerabilities, report.notices, report.withdrawn, report.new_affects
    );
    assert_eq!(report.load.advisory_count, 1552);
    assert_eq!(
        report.vulnerabilities + report.notices,
        1552,
        "every advisory has a kind"
    );
    assert_eq!(
        report.load.commit,
        "41c52cdab5fd303310c06be767cd48ad14970be9"
    );

    let ours = store_findings(&project.dir);
    let golden = recorded(fixture_name);
    assert_eq!(
        ours.0, golden.0,
        "vulnerabilities equal the recorded cargo audit result"
    );
    assert_eq!(
        ours.1, golden.1,
        "notices equal the recorded cargo audit result"
    );
    assert_eq!((ours.0.len(), ours.1.len()), expected_counts);

    match live_cargo_audit(&fixture_dir.join("Cargo.lock")) {
        Some(live) => {
            assert_eq!(ours, live, "findings equal cargo audit run now");
            println!("{fixture_name}: live cargo audit agrees");
        }
        None => eprintln!(
            "NOTE: live cargo audit comparison skipped for `{fixture_name}` (needs the advisory-db submodule and cargo-audit); the recorded result was used"
        ),
    }
}

#[test]
fn findings_on_small_equal_cargo_audit() {
    let fixture = require_fixture!("small");
    assert_equals_cargo_audit("small", &fixture, (3, 0));
}

#[test]
fn findings_on_bat_equal_cargo_audit() {
    let fixture = require_fixture!("bat");
    assert_equals_cargo_audit("bat", &fixture, (6, 7));
}

#[test]
fn loading_twice_adds_no_relationship_and_keeps_first_seen() {
    let fixture = require_fixture!("small");
    let project = project_from(&fixture);
    engine_at(&project.dir, T0).init().expect("init");
    let first = engine_at(&project.dir, T1)
        .load_advisories(&advisories_dir(), None)
        .expect("first load");
    assert_eq!(first.new_affects, 3);
    let second = engine_at(&project.dir, T2)
        .load_advisories(&advisories_dir(), None)
        .expect("second load");
    assert_eq!(second.new_affects, 0);
    assert_eq!(second.removed_affects, 0);
    assert_eq!(second.load.id, "load-0002");

    let mut store = Store::open_read_only(&project.dir).expect("open");
    let counts = store.counts().expect("counts");
    assert_eq!(counts["relationship:AFFECTS"], 3);
    assert_eq!(counts["node:Advisory"], 1552);
    assert_eq!(counts["node:AdvisoryLoad"], 2, "every load is recorded");
    let baseline = store.baseline().expect("baseline").expect("has baseline");
    for finding in store.findings(&baseline.id).expect("findings") {
        assert_eq!(
            finding.first_seen_at, "2026-11-27T10:00:00.000Z",
            "first seen is never moved"
        );
    }
}

#[test]
fn published_before_loads_only_earlier_advisories() {
    let fixture = require_fixture!("small");
    let project = project_from(&fixture);
    engine_at(&project.dir, T0).init().expect("init");
    let report = engine_at(&project.dir, T1)
        .load_advisories(&advisories_dir(), Some("2022-03-08"))
        .expect("load");
    assert_eq!(
        report.load.published_before.as_deref(),
        Some("2022-03-08T00:00:00.000Z")
    );
    assert_eq!(
        report.load.advisory_count + report.skipped_by_cutoff as i64,
        1552
    );
    let (vulnerabilities, _) = store_findings(&project.dir);
    let expected: Findings = [
        "RUSTSEC-2020-0071 time 0.1.43",
        "RUSTSEC-2021-0003 smallvec 1.6.0",
    ]
    .map(String::from)
    .into();
    assert_eq!(
        vulnerabilities, expected,
        "the regex advisory of 2022-03-08 is not loaded yet"
    );
}

/// Writes one synthetic advisory against `regex` into a fresh directory.
fn write_advisory(dir: &Path, fixed: &str, modified: &str, withdrawn: Option<&str>) {
    fs::create_dir_all(dir).expect("dir");
    let mut record = serde_json::json!({
        "id": "TEST-2026-0001",
        "published": "2026-01-01T12:00:00Z",
        "modified": modified,
        "summary": "A synthetic advisory",
        "affected": [{
            "package": {"ecosystem": "crates.io", "name": "regex"},
            "database_specific": {"informational": null},
            "ranges": [{"type": "SEMVER", "events": [{"introduced": "0.0.0-0"}, {"fixed": fixed}]}]
        }]
    });
    if let Some(at) = withdrawn {
        record["withdrawn"] = serde_json::json!(at);
    }
    fs::write(dir.join("TEST-2026-0001.json"), record.to_string()).expect("write advisory");
    fs::write(
        dir.join("SOURCE.txt"),
        "repository: synthetic\nbranch: test\ncommit: abc123\ndate: 2026-01-01\n",
    )
    .expect("write source");
}

#[test]
fn an_edited_advisory_gains_and_loses_versions() {
    let fixture = require_fixture!("small");
    let project = project_from(&fixture);
    let data = tempfile::tempdir().expect("tempdir");
    engine_at(&project.dir, T0).init().expect("init");

    // First it stops short of the fixture's regex 1.5.4.
    write_advisory(data.path(), "1.5.0", "2026-01-01T12:00:00Z", None);
    let first = engine_at(&project.dir, T1)
        .load_advisories(data.path(), None)
        .expect("load");
    assert_eq!(first.new_affects, 0);
    assert_eq!(first.load.repository, "synthetic");
    assert!(store_findings(&project.dir).0.is_empty());

    // Then it is edited to cover more versions: the pair is first seen now.
    write_advisory(data.path(), "1.6.0", "2026-02-01T12:00:00Z", None);
    let second = engine_at(&project.dir, T2)
        .load_advisories(data.path(), None)
        .expect("load");
    assert_eq!(second.new_affects, 1);
    let mut store = Store::open_read_only(&project.dir).expect("open");
    let baseline = store.baseline().expect("baseline").expect("has baseline");
    let findings = store.findings(&baseline.id).expect("findings");
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].purl, "pkg:cargo/regex@1.5.4");
    assert_eq!(findings[0].first_seen_at, "2026-11-27T11:00:00.000Z");
    assert_eq!(
        findings[0].advisory.published, "2026-01-01T12:00:00.000Z",
        "the publication date is kept"
    );
    drop(store);

    // Narrowed again: the stale relationship is removed.
    write_advisory(data.path(), "1.5.0", "2026-03-01T12:00:00Z", None);
    let third = engine_at(&project.dir, "2026-11-27T12:00:00Z")
        .load_advisories(data.path(), None)
        .expect("load");
    assert_eq!(third.removed_affects, 1);
    assert!(store_findings(&project.dir).0.is_empty());
}

#[test]
fn load_from_the_command_line() {
    let fixture = require_fixture!("small");
    let project = project_from(&fixture);
    assert_eq!(run_cli(&project.dir, Some(T0), &["init"]).code, 0);
    let dir = advisories_dir();
    let dir = dir.to_str().unwrap();

    let output = run_cli(
        &project.dir,
        Some(T1),
        &[
            "advisories",
            "load",
            dir,
            "--published-before",
            "2022-03-08",
            "--json",
        ],
    );
    assert_eq!(output.code, 0, "{}", output.stderr);
    let report = output.json();
    assert_eq!(report["load"]["id"], "load-0001");
    assert_eq!(report["new_affects"], 2);

    let text = run_cli(
        &project.dir,
        Some(T2),
        &[
            "advisories",
            "load",
            dir,
            "--published-before",
            "2022-03-08",
        ],
    );
    assert!(
        text.stdout.contains("simulating the passage of time"),
        "{}",
        text.stdout
    );

    let missing = run_cli(
        &project.dir,
        Some(T2),
        &["advisories", "load", "/nonexistent/advisories"],
    );
    assert_eq!(missing.code, 4);
}
