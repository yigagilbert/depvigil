//! Test 2: two snapshots and a Cypher diff.
//!
//! Loads the `public` fixture as a baseline snapshot and a hand-edited copy
//! as a candidate snapshot into one Relate directory store, then checks that
//! Cypher produces the same added, removed and changed sets as plain Rust,
//! and that a dependency path to an added version is real.
//!
//! Run: `cargo test --test t2_snapshots -- --nocapture`

mod common;

use std::collections::BTreeSet;
use std::fs;
use std::time::Instant;

use common::graph::{
    create_schema, load_snapshot, params, resolve_copy, size_on_disk, string_column, string_list,
    text, SnapshotData, DIFF_ADDED, DIFF_CHANGED, DIFF_REMOVED,
};
use common::{file_sha256, find_named, fixture, git_status};
use relate::{Connection, Database, Value};

const BASE: &str = "bat-baseline";
const CAND: &str = "bat-candidate";

/// The brief's sketch: a property map on the variable-length relationship.
const PATH_PROPERTY: &str = "
MATCH p = (root:Version {purl: $root})
          -[:DEPENDS_ON*1..25 {snapshot: $cand}]->
          (v:Version {purl: $target})
RETURN [n IN nodes(p) | n.purl] AS purls,
       [r IN relationships(p) | r.snapshot] AS snapshots
LIMIT 5";

/// The same question as one shortest path.
const PATH_SHORTEST: &str = "
MATCH p = SHORTEST 1 (root:Version {purl: $root})
          (()-[r:DEPENDS_ON WHERE r.snapshot = $cand]->()){1,25}
          (v:Version {purl: $target})
RETURN [n IN nodes(p) | n.purl] AS purls,
       [r IN relationships(p) | r.snapshot] AS snapshots";

const COUNT_VERSIONS: &str = "MATCH (v:Version) RETURN count(v) AS n";
const COUNT_SHARED: &str = "
MATCH (:Snapshot {id: $base})-[:INCLUDES]->(v:Version)<-[:INCLUDES]-(:Snapshot {id: $cand})
RETURN count(v) AS n";
const COUNT_PURL: &str = "MATCH (v:Version {purl: $purl}) RETURN count(v) AS n";
const COUNT_EDGES: &str =
    "MATCH (:Version)-[r:DEPENDS_ON {snapshot: $snapshot}]->(:Version) RETURN count(r) AS n";

fn snapshot_params() -> std::collections::BTreeMap<String, Value> {
    params([("base", text(BASE)), ("cand", text(CAND))])
}

fn int(conn: &mut Connection, query: &str, p: std::collections::BTreeMap<String, Value>) -> i64 {
    conn.query_with_params(query, p).expect("count query").rows[0][0]
        .as_int()
        .expect("integer")
}

/// Runs a query, prints wall and engine time, and returns the first column.
fn timed(conn: &mut Connection, name: &str, query: &str, checks: &mut Vec<(String, bool)>) -> Vec<String> {
    let start = Instant::now();
    let result = conn.query_with_params(query, snapshot_params()).expect(name);
    let wall = start.elapsed();
    println!(
        "[t2] {name} query: wall {wall:?}, engine {:.3} ms, rows {}",
        result.elapsed_ms(),
        result.rows.len()
    );
    checks.push((format!("speed: {name} query under 1 second"), wall.as_secs_f64() < 1.0));
    string_column(&result.rows)
}

#[test]
fn t2_two_snapshots_and_diff() {
    let mut checks: Vec<(String, bool)> = Vec::new();
    let fixture_root = fixture("public");
    let lock_before = file_sha256(&fixture_root.join("Cargo.lock"));

    // Steps 1 and 2: resolve the baseline and a hand-edited candidate, each
    // in its own temporary copy.
    let (base, _base_dir) = resolve_copy(&fixture_root, |_| {});
    let (cand, cand_dir) = resolve_copy(&fixture_root, |dir| {
        let manifest = dir.join("Cargo.toml");
        let text = fs::read_to_string(&manifest).expect("read manifest");
        let from = "[dependencies.git2]\nversion = \"0.19\"";
        let to = "[dependencies.git2]\nversion = \"0.20\"";
        assert!(text.contains(from), "expected the git2 0.19 requirement");
        fs::write(&manifest, text.replace(from, to)).expect("write manifest");
    });
    println!("[t2] hand edit in the candidate copy: git2 requirement \"0.19\" -> \"0.20\"");
    println!(
        "[t2] baseline: {} versions, {} edges; candidate: {} versions, {} edges",
        base.versions.len(),
        base.edges.len(),
        cand.versions.len(),
        cand.edges.len()
    );
    assert!(find_named(cand_dir.path(), "target").is_empty(), "nothing may be compiled");

    // Step 3: the expected difference in plain Rust.
    let base_purls = base.purls();
    let cand_purls = cand.purls();
    let expect_added: Vec<String> = cand_purls.difference(&base_purls).cloned().collect();
    let expect_removed: Vec<String> = base_purls.difference(&cand_purls).cloned().collect();
    let package_of = |data: &SnapshotData, purl: &str| {
        data.versions.iter().find(|v| v.purl == purl).expect("version").package.clone()
    };
    let mut expect_changed = BTreeSet::new();
    for old in &expect_removed {
        for new in &expect_added {
            if package_of(&base, old) == package_of(&cand, new) {
                expect_changed.insert(format!("{} {old} -> {new}", package_of(&base, old)));
            }
        }
    }
    let expect_changed: Vec<String> = expect_changed.into_iter().collect();
    println!(
        "[t2] expected from plain Rust: {} added, {} removed, {} changed",
        expect_added.len(),
        expect_removed.len(),
        expect_changed.len()
    );
    println!("[t2]   added: {}", expect_added.join(", "));
    println!("[t2]   removed: {}", expect_removed.join(", "));
    println!("[t2]   changed: {}", expect_changed.join(", "));
    assert!(!expect_added.is_empty() && !expect_removed.is_empty() && !expect_changed.is_empty());

    // Load both snapshots into one directory store.
    let store_dir = tempfile::tempdir().expect("tempdir");
    let store = store_dir.path().join("t2_store");
    let db = Database::open(store.to_str().expect("utf-8 path")).expect("open directory store");
    let mut conn = db.connect().expect("connect");
    create_schema(&mut conn);

    let load_a = load_snapshot(&mut conn, "bat", BASE, "baseline", "2026-11-27T00:00:00Z", &base);
    db.checkpoint().expect("checkpoint");
    let size_one = size_on_disk(&store);
    println!("[t2] load baseline: {load_a:?}; store size with one snapshot (after checkpoint): {size_one} bytes");

    let load_b = load_snapshot(&mut conn, "bat", CAND, "candidate", "2026-11-27T00:00:01Z", &cand);
    db.checkpoint().expect("checkpoint");
    let size_two = size_on_disk(&store);
    println!("[t2] load candidate: {load_b:?}; store size with two snapshots (after checkpoint): {size_two} bytes");

    // Step 4: the same three sets with Cypher.
    let added = timed(&mut conn, "added", DIFF_ADDED, &mut checks);
    let removed = timed(&mut conn, "removed", DIFF_REMOVED, &mut checks);
    let changed = timed(&mut conn, "changed", DIFF_CHANGED, &mut checks);
    checks.push(("added: Cypher equals plain Rust".into(), added == expect_added));
    checks.push(("removed: Cypher equals plain Rust".into(), removed == expect_removed));
    checks.push(("changed: Cypher equals plain Rust".into(), changed == expect_changed));

    // Shared nodes: a version in both snapshots is one node.
    let union: BTreeSet<&String> = base_purls.union(&cand_purls).collect();
    let shared = base_purls.intersection(&cand_purls).count() as i64;
    let total_nodes = int(&mut conn, COUNT_VERSIONS, params([]));
    let shared_nodes = int(&mut conn, COUNT_SHARED, snapshot_params());
    let sample = base_purls.intersection(&cand_purls).next().expect("a shared version");
    let sample_nodes = int(&mut conn, COUNT_PURL, params([("purl", text(sample))]));
    println!(
        "[t2] Version nodes: {total_nodes} (distinct purls across both snapshots: {}); included by both: {shared_nodes} (expected {shared}); nodes for {sample}: {sample_nodes}",
        union.len()
    );
    checks.push((
        "shared nodes: one node per version across snapshots".into(),
        total_nodes == union.len() as i64 && shared_nodes == shared && sample_nodes == 1,
    ));
    let edges_a = int(&mut conn, COUNT_EDGES, params([("snapshot", text(BASE))]));
    let edges_b = int(&mut conn, COUNT_EDGES, params([("snapshot", text(CAND))]));
    println!("[t2] DEPENDS_ON edges: baseline {edges_a}, candidate {edges_b}");
    assert_eq!(edges_a, base.edges.len() as i64);
    assert_eq!(edges_b, cand.edges.len() as i64);

    // Step 5: a dependency path from the workspace root to an added version.
    let root = cand.roots[0].clone();
    let target = expect_added
        .iter()
        .find(|purl| !cand.edges.iter().any(|(from, to, _)| from == &root && to == *purl))
        .unwrap_or(&expect_added[0])
        .clone();
    println!("[t2] path question: why is {target} in the candidate? (root {root})");
    let cand_edges: BTreeSet<(String, String)> =
        cand.edges.iter().map(|(from, to, _)| (from.clone(), to.clone())).collect();
    for (name, query) in [("path (property map, LIMIT 5)", PATH_PROPERTY), ("path (SHORTEST 1)", PATH_SHORTEST)] {
        let start = Instant::now();
        let outcome = conn.query_with_params(
            query,
            params([("root", text(&root)), ("target", text(&target)), ("cand", text(CAND))]),
        );
        let wall = start.elapsed();
        match outcome {
            Err(error) => {
                println!("[t2] {name}: query rejected: {error}");
                checks.push((format!("{name}: query runs"), false));
            }
            Ok(result) => {
                println!(
                    "[t2] {name}: wall {wall:?}, engine {:.3} ms, paths {}",
                    result.elapsed_ms(),
                    result.rows.len()
                );
                let mut all_real = !result.rows.is_empty();
                for row in &result.rows {
                    let purls = string_list(&row[0]);
                    let snapshots = string_list(&row[1]);
                    println!("[t2]   {}", purls.join(" -> "));
                    let real = purls.first() == Some(&root)
                        && purls.last() == Some(&target)
                        && purls
                            .windows(2)
                            .all(|pair| cand_edges.contains(&(pair[0].clone(), pair[1].clone())))
                        && snapshots.iter().all(|s| s == CAND);
                    all_real &= real;
                }
                checks.push((
                    format!("{name}: every returned path exists edge by edge in snapshot B"),
                    all_real,
                ));
                checks.push((format!("speed: {name} under 1 second"), wall.as_secs_f64() < 1.0));
            }
        }
    }

    // The fixture is untouched.
    checks.push((
        "fixture untouched".into(),
        git_status(&fixture_root).is_empty() && file_sha256(&fixture_root.join("Cargo.lock")) == lock_before,
    ));

    for (name, ok) in &checks {
        println!("[t2] {} {name}", if *ok { "PASS" } else { "FAIL" });
    }
    assert!(checks.iter().all(|(_, ok)| *ok), "Test 2 has failing checks");
}
