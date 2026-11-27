//! Test 4: advisory matching and Living Approvals.
//!
//! Part B runs first in the store because it needs the advisories loaded in
//! two stages. Part A then checks the fully loaded store against
//! `cargo audit`. The optional withdrawal check runs last because it edits
//! an advisory.
//!
//! Run: `cargo test --test t4_advisories -- --nocapture`

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use common::graph::{
    create_schema, load_snapshot, params, resolve_copy, shared_cargo_home, string_list, text,
    CRATES_IO,
};
use common::{file_sha256, find_named, fixture, git_status, run_cargo, BUILD_MARKER};
use relate::{Connection, Database, Value};
use serde_json::Value as Json;

type Params = BTreeMap<String, Value>;

/// The advisory whose publication invalidates the target approval.
const TARGET_ADVISORY: &str = "RUSTSEC-2022-0013";
const TARGET_PURL: &str = "pkg:cargo/regex@1.5.4";

const SNAP_BASE: &str = "small-baseline";
const SNAP_TARGET: &str = "small-target";
const SNAP_CONTROL: &str = "small-control";
const DECISION_TARGET: &str = "decision-target";
const DECISION_CONTROL: &str = "decision-control";

// ---- OSV advisories and range evaluation -----------------------------------

struct Advisory {
    id: String,
    published: String,
    modified: String,
    withdrawn: Option<String>,
    /// `unmaintained`, `unsound` or `notice`; `None` for a vulnerability.
    informational: Option<String>,
    /// Affected crates.io package name with its OSV SEMVER ranges. Each range
    /// is a list of `(is_fixed, version)` events.
    affected: Vec<(String, Vec<Vec<(bool, semver::Version)>>)>,
}

fn load_osv(dir: &Path) -> Vec<Advisory> {
    let mut advisories = Vec::new();
    for entry in fs::read_dir(dir).expect("read advisories") {
        let path = entry.expect("entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let json: Json = serde_json::from_str(&fs::read_to_string(&path).expect("read")).expect("OSV JSON");
        let string = |value: &Json| value.as_str().map(str::to_string);
        let mut informational = None;
        let mut affected = Vec::new();
        for item in json["affected"].as_array().expect("affected") {
            assert_eq!(item["package"]["ecosystem"].as_str(), Some("crates.io"));
            informational = informational.or(string(&item["database_specific"]["informational"]));
            let mut ranges = Vec::new();
            for range in item["ranges"].as_array().expect("ranges") {
                assert_eq!(range["type"].as_str(), Some("SEMVER"), "only SEMVER ranges are handled");
                let mut events = Vec::new();
                for event in range["events"].as_array().expect("events") {
                    let (fixed, version) = match (event.get("introduced"), event.get("fixed")) {
                        (Some(v), None) => (false, v),
                        (None, Some(v)) => (true, v),
                        _ => panic!("unsupported OSV event in {}: {event}", path.display()),
                    };
                    let version = semver::Version::parse(version.as_str().expect("event version"))
                        .expect("semver event");
                    events.push((fixed, version));
                }
                ranges.push(events);
            }
            affected.push((string(&item["package"]["name"]).expect("package name"), ranges));
        }
        advisories.push(Advisory {
            id: string(&json["id"]).expect("id"),
            published: string(&json["published"]).expect("published"),
            modified: string(&json["modified"]).expect("modified"),
            withdrawn: string(&json["withdrawn"]),
            informational,
            affected,
        });
    }
    advisories.sort_by(|a, b| a.id.cmp(&b.id));
    advisories
}

/// OSV range evaluation: walk the events in version order; an `introduced`
/// at or below the version turns "affected" on, a `fixed` at or below it
/// turns it off.
fn in_range(events: &[(bool, semver::Version)], version: &semver::Version) -> bool {
    let mut events: Vec<&(bool, semver::Version)> = events.iter().collect();
    events.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));
    let mut affected = false;
    for (fixed, at) in events {
        if at <= version {
            affected = !fixed;
        }
    }
    affected
}

fn affects(advisory: &Advisory, name: &str, version: &semver::Version) -> bool {
    advisory
        .affected
        .iter()
        .any(|(package, ranges)| package == name && ranges.iter().any(|r| in_range(r, version)))
}

/// `(advisory id, package, version)` triples.
type Findings = BTreeSet<(String, String, String)>;

/// Our matcher over a list of crates.io packages, split into vulnerabilities
/// and informational notices. Withdrawn advisories are skipped.
fn match_packages(advisories: &[Advisory], packages: &[(String, String)]) -> (Findings, Findings) {
    let mut by_name: BTreeMap<&str, Vec<&Advisory>> = BTreeMap::new();
    for advisory in advisories.iter().filter(|a| a.withdrawn.is_none()) {
        for (name, _) in &advisory.affected {
            by_name.entry(name).or_default().push(advisory);
        }
    }
    let (mut vulnerabilities, mut notices) = (Findings::new(), Findings::new());
    for (name, version) in packages {
        let parsed = semver::Version::parse(version).expect("package version");
        for advisory in by_name.get(name.as_str()).into_iter().flatten() {
            if affects(advisory, name, &parsed) {
                let finding = (advisory.id.clone(), name.clone(), version.clone());
                if advisory.informational.is_some() {
                    notices.insert(finding);
                } else {
                    vulnerabilities.insert(finding);
                }
            }
        }
    }
    (vulnerabilities, notices)
}

fn lock_registry_packages(lock: &Path) -> Vec<(String, String)> {
    let table: toml::Table = fs::read_to_string(lock).expect("read lock").parse().expect("parse lock");
    table["package"]
        .as_array()
        .expect("packages")
        .iter()
        .filter(|p| p.get("source").and_then(|s| s.as_str()) == Some(CRATES_IO))
        .map(|p| {
            (
                p["name"].as_str().expect("name").to_string(),
                p["version"].as_str().expect("version").to_string(),
            )
        })
        .collect()
}

/// Ground truth: `cargo audit --json` on a lockfile with the pinned
/// advisory database and no fetching. Returns vulnerabilities and the
/// advisory-backed warnings (unmaintained, unsound, notice).
fn cargo_audit(lock: &Path, db: &Path) -> (Findings, Findings) {
    let output = Command::new("cargo")
        .args(["audit", "--json", "--no-fetch", "--db"])
        .arg(db)
        .arg("--file")
        .arg(lock)
        .output()
        .expect("run cargo audit");
    let report: Json = serde_json::from_slice(&output.stdout).expect("cargo audit JSON");
    let finding = |item: &Json| {
        item["advisory"]["id"].as_str().map(|id| {
            (
                id.to_string(),
                item["package"]["name"].as_str().expect("name").to_string(),
                item["package"]["version"].as_str().expect("version").to_string(),
            )
        })
    };
    let vulnerabilities = report["vulnerabilities"]["list"]
        .as_array()
        .expect("vulnerability list")
        .iter()
        .filter_map(finding)
        .collect();
    let notices = report["warnings"]
        .as_object()
        .expect("warnings")
        .values()
        .flat_map(|list| list.as_array().expect("warning list").iter())
        .filter_map(finding)
        .collect();
    (vulnerabilities, notices)
}

/// RUSTSEC ids present as advisory files in the pinned advisory-db checkout.
fn advisory_db_ids(db: &Path) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    let mut stack = vec![db.join("crates")];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(dir).expect("read advisory-db") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                stack.push(path);
            } else if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                if stem.starts_with("RUSTSEC-") {
                    ids.insert(stem.to_string());
                }
            }
        }
    }
    ids
}

// ---- Cypher ----------------------------------------------------------------

const LOAD_ADVISORIES: &str = "
UNWIND $rows AS row
MERGE (a:Advisory {id: row.id})
SET a.published = datetime(row.published),
    a.modified = datetime(row.modified),
    a.withdrawn = datetime(row.withdrawn),
    a.informational = row.informational";

const ALL_VERSIONS: &str = "MATCH (v:Version) RETURN v.purl AS purl, v.source AS source ORDER BY purl";

const CREATE_AFFECTS: &str = "
UNWIND $pairs AS pair
MATCH (a:Advisory {id: pair.advisory})
MATCH (v:Version {purl: pair.purl})
MERGE (a)-[:AFFECTS]->(v)";

/// Both approvals are dated one day before D, the target advisory's
/// publication date.
const CREATE_DECISION: &str = "
MATCH (base:Snapshot {id: $base})
MATCH (cand:Snapshot {id: $cand})
MERGE (policy:Policy {id: $policy}) ON CREATE SET policy.rule = $rule
CREATE (d:Decision {id: $id, verdict: 'approved',
                    decided_at: datetime($d) - duration('P1D'),
                    digest: cand.digest})
CREATE (d)-[:COMPARES]->(base)
CREATE (d)-[:APPROVES]->(cand)
CREATE (d)-[:UNDER]->(policy)";

/// The brief's sketch, with the snapshot id added to the output.
const FIND_INVALIDATED: &str = "
MATCH (d:Decision {verdict: 'approved'})-[:APPROVES]->(s:Snapshot)
      -[:INCLUDES]->(v:Version)<-[:AFFECTS]-(a:Advisory)
WHERE a.published > d.decided_at
  AND a.withdrawn IS NULL
  AND NOT EXISTS { (:Invalidation)-[:INVALIDATES]->(d) }
RETURN d.id AS decision, a.id AS advisory, v.purl AS purl, s.id AS snapshot
ORDER BY decision, advisory, purl";

const CREATE_INVALIDATION: &str = "
MATCH (d:Decision {id: $decision})
MATCH (a:Advisory {id: $advisory})
CREATE (i:Invalidation {id: $id, at: datetime($at), reason: $reason})
CREATE (i)-[:INVALIDATES]->(d)
CREATE (i)-[:CAUSED_BY]->(a)";

const DECISION_NODE: &str = "MATCH (d:Decision {id: $id}) RETURN d";
const DECISION_DATES: &str = "
MATCH (d:Decision) RETURN d.id AS id, toString(d.decided_at) AS decided_at ORDER BY id";
const ADVISORY_DATE: &str =
    "MATCH (a:Advisory {id: $id}) RETURN toString(a.published) AS published";
const COUNT_INVALIDATIONS: &str = "MATCH (i:Invalidation) RETURN count(i) AS n";
const COUNT_INVALIDATIONS_OF: &str =
    "MATCH (i:Invalidation)-[:INVALIDATES]->(:Decision {id: $id}) RETURN count(i) AS n";
const COUNT_ADVISORIES: &str = "MATCH (a:Advisory) RETURN count(a) AS n";
const COUNT_AFFECTS: &str = "MATCH (:Advisory)-[r:AFFECTS]->(:Version) RETURN count(r) AS n";

const EXPLAIN_PATH: &str = "
MATCH (:Invalidation)-[:INVALIDATES]->(d:Decision {id: $decision})
      -[:APPROVES]->(s:Snapshot)-[:HAS_ROOT]->(root:Version)
MATCH p = (root)-[:DEPENDS_ON*1..25 {snapshot: $snapshot}]->(v:Version {purl: $purl})
WHERE s.id = $snapshot
RETURN [n IN nodes(p) | n.purl] AS purls
ORDER BY size(purls) LIMIT 1";

const SNAPSHOT_FINDINGS: &str = "
MATCH (:Snapshot {id: $snapshot})-[:INCLUDES]->(v:Version)<-[:AFFECTS]-(a:Advisory)
WHERE a.withdrawn IS NULL
RETURN a.id AS advisory, v.purl AS purl, a.informational AS informational
ORDER BY advisory, purl";

const WITHDRAW: &str = "MATCH (a:Advisory {id: $id}) SET a.withdrawn = datetime($at)";

/// Decisions whose every invalidation is caused by a withdrawn advisory.
const VALID_AGAIN: &str = "
MATCH (i:Invalidation)-[:INVALIDATES]->(d:Decision)
MATCH (i)-[:CAUSED_BY]->(a:Advisory)
WITH d, count(a) AS causes, count(a.withdrawn) AS withdrawn
WHERE causes = withdrawn
RETURN d.id AS decision ORDER BY decision";

fn int(conn: &mut Connection, query: &str, p: Params) -> i64 {
    conn.query_with_params(query, p).expect("count query").rows[0][0]
        .as_int()
        .expect("integer")
}

fn optional_text(value: &Option<String>) -> Value {
    value.as_deref().map(text).unwrap_or(Value::Null)
}

fn load_advisories(conn: &mut Connection, advisories: &[&Advisory]) -> Duration {
    let start = Instant::now();
    let rows: Vec<Value> = advisories
        .iter()
        .map(|a| {
            Value::Map(
                params([
                    ("id", text(&a.id)),
                    ("published", text(&a.published)),
                    ("modified", text(&a.modified)),
                    ("withdrawn", optional_text(&a.withdrawn)),
                    ("informational", optional_text(&a.informational)),
                ])
                .into(),
            )
        })
        .collect();
    conn.query_with_params(LOAD_ADVISORIES, params([("rows", Value::List(rows.into()))]))
        .expect("load advisories");
    start.elapsed()
}

/// `name` and `version` from a crates.io purl such as `pkg:cargo/regex@1.5.4`.
fn name_version(purl: &str) -> (String, String) {
    let rest = purl.strip_prefix("pkg:cargo/").expect("cargo purl");
    let (name, version) = rest.split_once('@').expect("versioned purl");
    (name.to_string(), version.to_string())
}

/// Evaluates the given advisories against every crates.io `Version` in the
/// store and creates `AFFECTS` edges. Returns (edges requested, time).
fn match_store(conn: &mut Connection, advisories: &[&Advisory]) -> (usize, Duration) {
    let start = Instant::now();
    let versions = conn.query(ALL_VERSIONS).expect("all versions").rows;
    let mut pairs = Vec::new();
    for row in &versions {
        if row[1].as_string() != Some(CRATES_IO) {
            continue; // identity includes the source: only crates.io versions match
        }
        let purl = row[0].as_string().expect("purl");
        let (name, version) = name_version(purl);
        let parsed = semver::Version::parse(&version).expect("version");
        for advisory in advisories {
            if affects(advisory, &name, &parsed) {
                pairs.push(Value::Map(
                    params([("advisory", text(&advisory.id)), ("purl", text(purl))]).into(),
                ));
            }
        }
    }
    let count = pairs.len();
    conn.query_with_params(CREATE_AFFECTS, params([("pairs", Value::List(pairs.into()))]))
        .expect("create AFFECTS");
    (count, start.elapsed())
}

fn report_agreement(
    name: &str,
    ours: &(Findings, Findings),
    audit: &(Findings, Findings),
    checks: &mut Vec<(String, bool)>,
) {
    let show = |set: &Findings| {
        set.iter()
            .map(|(id, package, version)| format!("{id} {package} {version}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    println!(
        "[t4] {name}: vulnerabilities ours {} / cargo audit {}; notices ours {} / cargo audit {}",
        ours.0.len(),
        audit.0.len(),
        ours.1.len(),
        audit.1.len()
    );
    println!("[t4]   vulnerabilities (cargo audit): {}", show(&audit.0));
    println!("[t4]   notices (cargo audit): {}", show(&audit.1));
    for (label, a, b) in [("vulnerabilities", &ours.0, &audit.0), ("notices", &ours.1, &audit.1)] {
        let only_ours: Findings = a.difference(b).cloned().collect();
        let only_audit: Findings = b.difference(a).cloned().collect();
        if !only_ours.is_empty() {
            println!("[t4]   {label} only ours: {}", show(&only_ours));
        }
        if !only_audit.is_empty() {
            println!("[t4]   {label} only cargo audit: {}", show(&only_audit));
        }
    }
    checks.push((format!("{name}: vulnerabilities equal cargo audit"), ours.0 == audit.0));
    checks.push((format!("{name}: informational notices equal cargo audit"), ours.1 == audit.1));
}

#[test]
fn t4_advisory_matching_and_living_approvals() {
    let mut checks: Vec<(String, bool)> = Vec::new();
    let small = fixture("small");
    let public = fixture("public");
    let advisory_db = fixture("advisory-db");
    let small_lock = small.join("Cargo.lock");
    let lock_before = file_sha256(&small_lock);

    // ---- Advisory data -----------------------------------------------------
    let advisories = load_osv(&fixture("advisories").join("osv"));
    let osv_ids: BTreeSet<String> = advisories.iter().map(|a| a.id.clone()).collect();
    let db_ids = advisory_db_ids(&advisory_db);
    let only_osv: Vec<&String> = osv_ids.difference(&db_ids).collect();
    let only_db: Vec<&String> = db_ids.difference(&osv_ids).collect();
    println!(
        "[t4] OSV snapshot: {} advisories ({} withdrawn, {} informational); advisory-db checkout: {} advisories",
        advisories.len(),
        advisories.iter().filter(|a| a.withdrawn.is_some()).count(),
        advisories.iter().filter(|a| a.informational.is_some()).count(),
        db_ids.len()
    );
    println!("[t4] only in the OSV snapshot: {only_osv:?}; only in the advisory-db checkout: {only_db:?}");

    // ---- Snapshots ---------------------------------------------------------
    let home = shared_cargo_home();
    let add = |dir: &Path, args: &[&str]| {
        let run = run_cargo(dir, &home, args);
        assert!(run.success, "cargo {} failed: {}", args.join(" "), run.stderr);
    };
    let (baseline, _d0) = resolve_copy(&small, |_| {});
    let (target, d1) = resolve_copy(&small, |dir| add(dir, &["add", "either@=1.13.0", "-p", "small-core"]));
    let (control, d2) = resolve_copy(&small, |dir| {
        add(dir, &["add", "either@=1.13.0", "-p", "small-core"]);
        add(dir, &["add", "regex@=1.5.5", "-p", "small-app"]);
    });
    for dir in [d1.path(), d2.path()] {
        assert!(find_named(dir, "target").is_empty() && find_named(dir, BUILD_MARKER).is_empty());
    }
    let only_target: Vec<String> = target.purls().difference(&control.purls()).cloned().collect();
    let only_control: Vec<String> = control.purls().difference(&target.purls()).cloned().collect();
    println!("[t4] target snapshot only: {only_target:?}; control snapshot only: {only_control:?}");

    let store_dir = tempfile::tempdir().expect("tempdir");
    let store = store_dir.path().join("t4_store");
    let db = Database::open(store.to_str().expect("utf-8 path")).expect("open directory store");
    let mut conn = db.connect().expect("connect");
    create_schema(&mut conn);
    load_snapshot(&mut conn, "small", SNAP_BASE, "baseline", "2022-03-01T00:00:00Z", &baseline);
    load_snapshot(&mut conn, "small", SNAP_TARGET, "candidate", "2022-03-07T00:00:00Z", &target);
    load_snapshot(&mut conn, "small", SNAP_CONTROL, "candidate", "2022-03-07T00:00:00Z", &control);

    // ---- Part B: invalidation ----------------------------------------------
    // Step 1: D is the publication date of the target advisory.
    let d = advisories
        .iter()
        .find(|a| a.id == TARGET_ADVISORY)
        .expect("target advisory")
        .published
        .clone();
    // Step 2: load only advisories published before D.
    let (before, rest): (Vec<&Advisory>, Vec<&Advisory>) =
        advisories.iter().partition(|a| a.published.as_str() < d.as_str());
    let load_before = load_advisories(&mut conn, &before);
    let (edges_before, match_before) = match_store(&mut conn, &before);
    println!(
        "[t4] D = {d}; loaded {} advisories published before D in {load_before:?}; matched in {match_before:?} ({edges_before} AFFECTS edges)",
        before.len()
    );

    // Steps 2 and 3: the target and control approvals, both dated the day
    // before D.
    for (id, cand) in [(DECISION_TARGET, SNAP_TARGET), (DECISION_CONTROL, SNAP_CONTROL)] {
        conn.query_with_params(
            CREATE_DECISION,
            params([
                ("id", text(id)),
                ("base", text(SNAP_BASE)),
                ("cand", text(cand)),
                ("policy", text("policy-manual-review")),
                ("rule", text("A human reviewed and approved the change")),
                ("d", text(&d)),
            ]),
        )
        .expect("create decision");
    }
    for row in conn.query(DECISION_DATES).expect("decision dates").rows {
        println!(
            "[t4] {} decided_at {}",
            row[0].as_string().unwrap_or("?"),
            row[1].as_string().unwrap_or("?")
        );
    }
    let fingerprint = |conn: &mut Connection| {
        let row = conn
            .query_with_params(DECISION_NODE, params([("id", text(DECISION_TARGET))]))
            .expect("decision node")
            .rows
            .remove(0);
        format!("{:?}", row[0])
    };
    let decision_before = fingerprint(&mut conn);

    // Before the new advisories arrive, nothing is invalidated.
    let early = conn.query(FIND_INVALIDATED).expect("invalidation query").rows;
    checks.push(("no decision is invalidated before the new advisory is loaded".into(), early.is_empty()));

    // Step 4: load the remaining advisories, as if the database had updated.
    let load_rest = load_advisories(&mut conn, &rest);
    let (edges_rest, match_rest) = match_store(&mut conn, &rest);
    println!(
        "[t4] loaded {} remaining advisories in {load_rest:?}; matched in {match_rest:?} ({edges_rest} AFFECTS edges)",
        rest.len()
    );
    let advisory_count = int(&mut conn, COUNT_ADVISORIES, params([]));
    let affects_count = int(&mut conn, COUNT_AFFECTS, params([]));
    let published = conn
        .query_with_params(ADVISORY_DATE, params([("id", text(TARGET_ADVISORY))]))
        .expect("advisory date")
        .rows
        .first()
        .and_then(|row| row[0].as_string())
        .unwrap_or("not loaded")
        .to_string();
    println!(
        "[t4] store now holds {advisory_count} advisories and {affects_count} AFFECTS edges; total advisory load {:?}, total matching {:?}; {TARGET_ADVISORY} published {published}",
        load_before + load_rest,
        match_before + match_rest
    );
    checks.push(("every OSV advisory is loaded".into(), advisory_count == advisories.len() as i64));

    // Step 5: run the invalidation query and record what it finds.
    let start = Instant::now();
    let found = conn.query(FIND_INVALIDATED).expect("invalidation query");
    println!(
        "[t4] invalidation query: wall {:?}, engine {:.3} ms, rows {}",
        start.elapsed(),
        found.elapsed_ms(),
        found.rows.len()
    );
    let rows: Vec<Vec<String>> = found
        .rows
        .iter()
        .map(|row| row.iter().map(|v| v.as_string().unwrap_or("?").to_string()).collect())
        .collect();
    for row in &rows {
        println!("[t4]   decision {} invalidated by {} via {} (snapshot {})", row[0], row[1], row[2], row[3]);
    }
    checks.push((
        "target found: the affected decision, with the advisory and the version".into(),
        rows.len() == 1
            && rows[0][0] == DECISION_TARGET
            && rows[0][1] == TARGET_ADVISORY
            && rows[0][2] == TARGET_PURL,
    ));
    checks.push((
        "control safe: the control decision is not returned".into(),
        !rows.iter().any(|row| row[0] == DECISION_CONTROL),
    ));
    let pairs: BTreeSet<(String, String)> = rows.iter().map(|r| (r[0].clone(), r[1].clone())).collect();
    for (decision, advisory) in &pairs {
        conn.query_with_params(
            CREATE_INVALIDATION,
            params([
                ("id", text(&format!("invalidation:{decision}:{advisory}"))),
                ("decision", text(decision)),
                ("advisory", text(advisory)),
                ("at", text("2026-11-27T12:00:00Z")),
                ("reason", text("An advisory published after the approval affects an approved version")),
            ]),
        )
        .expect("create invalidation");
    }

    // History kept.
    let decision_after = fingerprint(&mut conn);
    let pointing = int(&mut conn, COUNT_INVALIDATIONS_OF, params([("id", text(DECISION_TARGET))]));
    println!("[t4] decision node before: {decision_before}");
    println!("[t4] decision node after:  {decision_after}");
    checks.push((
        "history kept: Decision node unchanged, one Invalidation points at it".into(),
        decision_before == decision_after && pointing == 1,
    ));

    // Idempotent.
    let again = conn.query(FIND_INVALIDATED).expect("invalidation query").rows.len();
    let total = int(&mut conn, COUNT_INVALIDATIONS, params([]));
    println!("[t4] second run of the invalidation query: {again} rows; Invalidation nodes: {total}");
    checks.push(("idempotent: a second run finds nothing new".into(), again == 0 && total == 1));

    // Explained.
    let start = Instant::now();
    let explained = conn
        .query_with_params(
            EXPLAIN_PATH,
            params([
                ("decision", text(DECISION_TARGET)),
                ("snapshot", text(SNAP_TARGET)),
                ("purl", text(TARGET_PURL)),
            ]),
        )
        .expect("explain path");
    let path = explained.rows.first().map(|row| string_list(&row[0])).unwrap_or_default();
    println!("[t4] explanation path ({:?}): {}", start.elapsed(), path.join(" -> "));
    let target_edges: BTreeSet<(String, String)> =
        target.edges.iter().map(|(a, b, _)| (a.clone(), b.clone())).collect();
    checks.push((
        "explained: a real path from a workspace root to the affected version".into(),
        path.len() >= 2
            && target.roots.contains(&path[0])
            && path.last().map(String::as_str) == Some(TARGET_PURL)
            && path.windows(2).all(|p| target_edges.contains(&(p[0].clone(), p[1].clone()))),
    ));

    // ---- Part A: advisory matching against cargo audit ---------------------
    let graph_rows = conn
        .query_with_params(SNAPSHOT_FINDINGS, params([("snapshot", text(SNAP_BASE))]))
        .expect("snapshot findings")
        .rows;
    let mut graph = (Findings::new(), Findings::new());
    for row in &graph_rows {
        let (name, version) = name_version(row[1].as_string().expect("purl"));
        let finding = (row[0].as_string().expect("advisory").to_string(), name, version);
        if row[2].as_string().is_some() {
            graph.1.insert(finding);
        } else {
            graph.0.insert(finding);
        }
    }
    let audit_small = cargo_audit(&small_lock, &advisory_db);
    report_agreement("graph, small baseline snapshot", &graph, &audit_small, &mut checks);

    // Supplementary: the same evaluator over whole lockfiles, without the graph.
    let ours_small = match_packages(&advisories, &lock_registry_packages(&small_lock));
    report_agreement("evaluator, small Cargo.lock", &ours_small, &audit_small, &mut checks);
    let public_lock = public.join("Cargo.lock");
    let ours_public = match_packages(&advisories, &lock_registry_packages(&public_lock));
    let audit_public = cargo_audit(&public_lock, &advisory_db);
    report_agreement("evaluator, public Cargo.lock", &ours_public, &audit_public, &mut checks);

    // ---- Optional: withdrawal ----------------------------------------------
    let valid_before = conn.query(VALID_AGAIN).expect("valid again").rows.len();
    conn.query_with_params(
        WITHDRAW,
        params([("id", text(TARGET_ADVISORY)), ("at", text("2026-11-28T00:00:00Z"))]),
    )
    .expect("withdraw advisory");
    let valid_after: Vec<String> = conn
        .query(VALID_AGAIN)
        .expect("valid again")
        .rows
        .iter()
        .map(|row| row[0].as_string().unwrap_or("?").to_string())
        .collect();
    let decision_final = fingerprint(&mut conn);
    println!(
        "[t4] optional withdrawal: decisions valid again before {valid_before}, after {valid_after:?}; Invalidation nodes still {}",
        int(&mut conn, COUNT_INVALIDATIONS, params([]))
    );
    checks.push((
        "optional: a decision invalidated only by a withdrawn advisory is reported valid again".into(),
        valid_before == 0 && valid_after == [DECISION_TARGET] && decision_final == decision_before,
    ));

    // ---- Fixtures untouched ------------------------------------------------
    checks.push((
        "fixtures untouched".into(),
        git_status(&small).is_empty()
            && git_status(&public).is_empty()
            && git_status(&advisory_db).is_empty()
            && file_sha256(&small_lock) == lock_before,
    ));

    for (name, ok) in &checks {
        println!("[t4] {} {name}", if *ok { "PASS" } else { "FAIL" });
    }
    assert!(checks.iter().all(|(_, ok)| *ok), "Test 4 has failing checks");
}
