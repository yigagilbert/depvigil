//! Test 5: approval binding and drift detection.
//!
//! An approval is bound to one resolved state by a SHA-256 digest over a
//! canonical package list (name, version, source, checksum; sorted). The
//! digest is stored on the `Snapshot` and on the `Decision`. Later, the
//! digest is recomputed from a checkout's `cargo metadata` output and
//! compared. A mismatch is explained with the Test 2 diff queries.
//!
//! Run: `cargo test --test t5_binding -- --nocapture`

mod common;

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::time::Instant;

use common::graph::{
    create_schema, load_snapshot, params, resolve_dir, shared_cargo_home, string_column, text,
    SnapshotData, DIFF_ADDED, DIFF_CHANGED, DIFF_REMOVED,
};
use common::{
    add_stubs, copy_full, copy_manifests, file_sha256, find_named, fixture, git_status, run_cargo,
    BUILD_MARKER,
};
use relate::{Connection, Database};

const BASELINE: &str = "small-baseline";
const APPROVED: &str = "small-approved";
const DECISION: &str = "decision-add-either";

/// The change that is simulated and approved.
const APPROVED_CHANGE: &[&[&str]] = &[&["add", "either@=1.13.0", "-p", "small-core"]];

const CREATE_DECISION: &str = "
MATCH (base:Snapshot {id: $base})
MATCH (cand:Snapshot {id: $cand})
MERGE (policy:Policy {id: $policy}) ON CREATE SET policy.rule = $rule
CREATE (d:Decision {id: $id, verdict: 'approved', decided_at: datetime($at), digest: cand.digest})
CREATE (d)-[:COMPARES]->(base)
CREATE (d)-[:APPROVES]->(cand)
CREATE (d)-[:UNDER]->(policy)";

const APPROVED_DIGESTS: &str = "
MATCH (d:Decision {id: $id})-[:APPROVES]->(s:Snapshot)
RETURN d.digest AS decision_digest, s.digest AS snapshot_digest, s.id AS snapshot";

fn apply(dir: &Path, changes: &[&[&str]]) {
    for change in changes {
        let run = run_cargo(dir, &shared_cargo_home(), change);
        assert!(run.success, "cargo {} failed: {}", change.join(" "), run.stderr);
    }
}

/// A full checkout of the fixture at `dir` with `changes` applied, resolved.
fn checkout(fixture_root: &Path, dir: &Path, changes: &[&[&str]]) -> SnapshotData {
    fs::create_dir_all(dir).expect("create checkout dir");
    copy_full(fixture_root, dir);
    apply(dir, changes);
    let snapshot = resolve_dir(dir);
    assert!(find_named(dir, "target").is_empty(), "nothing may be compiled");
    assert!(find_named(dir, BUILD_MARKER).is_empty(), "no build script may run");
    snapshot
}

/// The Cypher diff between the approved snapshot and an observed one.
fn explain(conn: &mut Connection, observed: &str) -> Vec<String> {
    let mut lines = Vec::new();
    for (label, query) in [("added", DIFF_ADDED), ("removed", DIFF_REMOVED), ("changed", DIFF_CHANGED)] {
        let rows = conn
            .query_with_params(query, params([("base", text(APPROVED)), ("cand", text(observed))]))
            .expect("diff query")
            .rows;
        for item in string_column(&rows) {
            lines.push(format!("{label}: {item}"));
        }
    }
    lines
}

#[test]
fn t5_approval_binding_and_drift_detection() {
    let mut checks: Vec<(String, bool)> = Vec::new();
    let small = fixture("small");
    let lock_before = file_sha256(&small.join("Cargo.lock"));
    let work = tempfile::tempdir().expect("tempdir");

    // ---- Approval: simulate in a manifest-only copy, store the digest ------
    let baseline = checkout(&small, &work.path().join("baseline"), &[]);
    let simulation = work.path().join("simulation");
    fs::create_dir_all(&simulation).expect("create simulation dir");
    copy_manifests(&small, &simulation);
    add_stubs(&simulation);
    apply(&simulation, APPROVED_CHANGE);
    let approved = resolve_dir(&simulation);
    println!("[t5] canonical list of the approved state:");
    for line in approved.canonical_list().lines() {
        println!("[t5]   {line}");
    }
    let canonical = approved.canonical_list();
    let work_path = work.path().canonicalize().expect("canonical work dir");
    checks.push((
        "no absolute path enters the canonical list".into(),
        !canonical.contains(work_path.to_str().expect("utf-8"))
            && !canonical.contains("<ROOT>")
            && !canonical.contains("file://")
            && canonical.contains("small-app 0.1.0 path:app ")
            && canonical.contains("marker 0.1.0 path:vendor/marker "),
    ));

    let store = work.path().join("t5_store");
    let db = Database::open(store.to_str().expect("utf-8 path")).expect("open directory store");
    let mut conn = db.connect().expect("connect");
    create_schema(&mut conn);
    load_snapshot(&mut conn, "small", BASELINE, "baseline", "2026-11-27T09:00:00Z", &baseline);
    load_snapshot(&mut conn, "small", APPROVED, "candidate", "2026-11-27T09:01:00Z", &approved);
    conn.query_with_params(
        CREATE_DECISION,
        params([
            ("id", text(DECISION)),
            ("base", text(BASELINE)),
            ("cand", text(APPROVED)),
            ("policy", text("policy-manual-review")),
            ("rule", text("A human reviewed and approved the change")),
            ("at", text("2026-11-27T09:02:00Z")),
        ]),
    )
    .expect("create decision");

    // Verification always starts from what the store says was approved.
    let row = conn
        .query_with_params(APPROVED_DIGESTS, params([("id", text(DECISION))]))
        .expect("approved digests")
        .rows
        .remove(0);
    let approved_digest = row[0].as_string().expect("decision digest").to_string();
    println!("[t5] approved digest (Decision): {approved_digest}");
    checks.push((
        "digest stored on both the Snapshot and the Decision".into(),
        row[1].as_string() == Some(approved_digest.as_str()) && approved_digest == approved.digest(),
    ));

    // ---- Cases -------------------------------------------------------------
    struct Case {
        id: &'static str,
        label: &'static str,
        dir: &'static str,
        changes: &'static [&'static [&'static str]],
        expect_match: bool,
        /// A fragment the explanation must contain when a mismatch is expected.
        must_name: &'static str,
        counted: bool,
    }
    let cases = [
        Case {
            id: "1",
            label: "the approved change is applied exactly",
            dir: "repo-exact",
            changes: APPROVED_CHANGE,
            expect_match: true,
            must_name: "",
            counted: true,
        },
        Case {
            id: "2",
            label: "a different version of one crate is applied",
            dir: "repo-other-version",
            changes: &[&["add", "either@=1.12.0", "-p", "small-core"]],
            expect_match: false,
            must_name: "changed: pkg:cargo/either pkg:cargo/either@1.13.0 -> pkg:cargo/either@1.12.0",
            counted: true,
        },
        Case {
            id: "3",
            label: "an extra crate is added beyond the approval",
            dir: "repo-extra-crate",
            changes: &[
                &["add", "either@=1.13.0", "-p", "small-core"],
                &["add", "hex@=0.4.3", "-p", "small-app"],
            ],
            expect_match: false,
            must_name: "added: pkg:cargo/hex@0.4.3",
            counted: true,
        },
        Case {
            id: "4",
            label: "the same approved state in a different directory",
            dir: "elsewhere/nested/another-checkout",
            changes: APPROVED_CHANGE,
            expect_match: true,
            must_name: "",
            counted: true,
        },
        Case {
            id: "observation",
            label: "the approved crate is added to a different workspace member",
            dir: "repo-other-member",
            changes: &[&["add", "either@=1.13.0", "-p", "small-app"]],
            expect_match: true,
            must_name: "",
            counted: false,
        },
    ];

    let mut counted_ok = 0;
    let mut exact_dir = None;
    for case in &cases {
        let dir = work.path().join(case.dir);
        let observed = checkout(&small, &dir, case.changes);
        let digest = observed.digest();
        let matches = digest == approved_digest;
        println!(
            "[t5 case {}] {}: digest {} -> {}",
            case.id,
            case.label,
            digest,
            if matches { "MATCH" } else { "MISMATCH" }
        );
        let mut ok = matches == case.expect_match;
        if !matches {
            let snapshot_id = format!("observed-{}", case.id);
            load_snapshot(&mut conn, "small", &snapshot_id, "observed", "2026-11-27T10:00:00Z", &observed);
            let explanation = explain(&mut conn, &snapshot_id);
            for line in &explanation {
                println!("[t5 case {}]   {line}", case.id);
            }
            ok &= !explanation.is_empty() && explanation.iter().any(|line| line == case.must_name);
        }
        if case.id == "observation" {
            let edge_sets_equal = observed.edges == approved.edges;
            println!(
                "[t5 case observation] same package list: {}; same dependency edges: {edge_sets_equal}",
                observed.purls() == approved.purls()
            );
            let only_observed: BTreeSet<_> = observed.edges.difference(&approved.edges).collect();
            for (from, to, kind) in only_observed {
                println!("[t5 case observation]   edge only in the applied state: {from} -> {to} ({kind})");
            }
        }
        if case.counted {
            checks.push((format!("case {}: {}", case.id, case.label), ok));
            if ok {
                counted_ok += 1;
            }
        } else {
            println!(
                "[t5 case {}] not a pass criterion; digests {}",
                case.id,
                if matches { "match" } else { "differ" }
            );
        }
        if case.id == "1" {
            exact_dir = Some(dir);
        }
    }
    println!("[t5] cases: {counted_ok} of 4 give the expected result");

    // ---- Stability ---------------------------------------------------------
    let exact_dir = exact_dir.expect("case 1 directory");
    let digests: BTreeSet<String> = (0..10).map(|_| resolve_dir(&exact_dir).digest()).collect();
    println!("[t5] ten recomputations from cargo metadata gave {} distinct value(s)", digests.len());
    checks.push((
        "stability: ten recomputations give one value".into(),
        digests.len() == 1 && digests.contains(&approved_digest),
    ));

    // ---- Digest time on the public fixture ---------------------------------
    let public = fixture("public");
    let public_dir = work.path().join("public");
    fs::create_dir_all(&public_dir).expect("create public dir");
    copy_full(&public, &public_dir);
    let start = Instant::now();
    let snapshot = resolve_dir(&public_dir);
    let resolve_time = start.elapsed();
    let start = Instant::now();
    let digest = snapshot.digest();
    let digest_time = start.elapsed();
    println!(
        "[t5] public fixture: {} packages; cargo metadata plus parsing {resolve_time:?}; canonical list plus SHA-256 {digest_time:?}; digest {digest}",
        snapshot.versions.len()
    );

    checks.push((
        "fixtures untouched".into(),
        git_status(&small).is_empty()
            && git_status(&public).is_empty()
            && file_sha256(&small.join("Cargo.lock")) == lock_before,
    ));
    for (name, ok) in &checks {
        println!("[t5] {} {name}", if *ok { "PASS" } else { "FAIL" });
    }
    assert!(checks.iter().all(|(_, ok)| *ok), "Test 5 has failing checks");
}
