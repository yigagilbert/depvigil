//! Snapshot extraction from `cargo metadata` and loading into a Relate store
//! using the draft schema from the brief. Shared by Tests 2, 4 and 5.
//!
//! Every Cypher statement here is a constant. Values travel as parameters.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use relate::{Connection, Value};
use serde_json::Value as Json;

use super::{copy_full, parse_metadata, run_cargo, sha256_hex};

pub const CRATES_IO: &str = "registry+https://github.com/rust-lang/crates.io-index";

/// A Cargo home kept under `target/` so later runs start with a warm cache.
pub fn shared_cargo_home() -> PathBuf {
    let home = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("phase0-cargo-home");
    fs::create_dir_all(&home).expect("create shared cargo home");
    home
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct VersionRow {
    /// `pkg:cargo/<name>@<version>`, with a `?source=` qualifier for anything
    /// that is not from crates.io, so identity includes the source.
    pub purl: String,
    /// `pkg:cargo/<name>`
    pub package: String,
    pub name: String,
    pub version: String,
    /// The Cargo source string, or `path:<dir relative to the workspace root>`.
    pub source: String,
    /// The lockfile checksum. Empty for path dependencies.
    pub checksum: String,
}

#[derive(Debug, Clone)]
pub struct SnapshotData {
    pub versions: Vec<VersionRow>,
    /// `(from purl, to purl, kind)` with kind `normal`, `dev` or `build`.
    pub edges: BTreeSet<(String, String, String)>,
    /// Purls of the workspace members.
    pub roots: Vec<String>,
}

impl SnapshotData {
    pub fn purls(&self) -> BTreeSet<String> {
        self.versions.iter().map(|v| v.purl.clone()).collect()
    }

    /// The canonical list bound by an approval: one line per resolved package
    /// with name, version, source and checksum, sorted. Path dependencies
    /// appear with a path relative to the workspace root, never an absolute one.
    pub fn canonical_list(&self) -> String {
        let mut lines: Vec<String> = self
            .versions
            .iter()
            .map(|v| format!("{} {} {} {}", v.name, v.version, v.source, v.checksum))
            .collect();
        lines.sort();
        lines.join("\n") + "\n"
    }

    pub fn digest(&self) -> String {
        sha256_hex(self.canonical_list().as_bytes())
    }
}

fn lock_checksums(lock_text: &str) -> BTreeMap<(String, String, String), String> {
    let lock: toml::Table = lock_text.parse().expect("parse Cargo.lock");
    let mut checksums = BTreeMap::new();
    for package in lock["package"].as_array().expect("package array") {
        let field = |key: &str| {
            package
                .get(key)
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string()
        };
        checksums.insert(
            (field("name"), field("version"), field("source")),
            field("checksum"),
        );
    }
    checksums
}

/// Builds snapshot data from normalised `cargo metadata` output (absolute
/// workspace root already replaced by `<ROOT>`) and the matching lockfile.
pub fn snapshot_from(metadata: &Json, lock_text: &str) -> SnapshotData {
    let checksums = lock_checksums(lock_text);
    let mut rows: BTreeMap<String, VersionRow> = BTreeMap::new();
    for package in metadata["packages"].as_array().expect("packages") {
        let id = package["id"].as_str().expect("id").to_string();
        let name = package["name"].as_str().expect("name").to_string();
        let version = package["version"].as_str().expect("version").to_string();
        let cargo_source = package["source"].as_str().unwrap_or_default().to_string();
        let source = if cargo_source.is_empty() {
            let manifest = package["manifest_path"].as_str().expect("manifest_path");
            let dir = manifest
                .strip_prefix("<ROOT>")
                .expect("path dependency inside the workspace root")
                .trim_end_matches("Cargo.toml")
                .trim_matches('/');
            format!("path:{}", if dir.is_empty() { "." } else { dir })
        } else {
            cargo_source.clone()
        };
        let purl = if source == CRATES_IO {
            format!("pkg:cargo/{name}@{version}")
        } else {
            format!("pkg:cargo/{name}@{version}?source={source}")
        };
        let checksum = checksums
            .get(&(name.clone(), version.clone(), cargo_source))
            .cloned()
            .unwrap_or_default();
        rows.insert(
            id,
            VersionRow {
                purl,
                package: format!("pkg:cargo/{name}"),
                name,
                version,
                source,
                checksum,
            },
        );
    }

    let mut versions = Vec::new();
    let mut edges = BTreeSet::new();
    for node in metadata["resolve"]["nodes"].as_array().expect("nodes") {
        let from = &rows[node["id"].as_str().expect("node id")];
        versions.push(from.clone());
        for dep in node["deps"].as_array().expect("deps") {
            let to = &rows[dep["pkg"].as_str().expect("dep pkg")];
            for kind in dep["dep_kinds"].as_array().expect("dep_kinds") {
                let kind = kind["kind"].as_str().unwrap_or("normal").to_string();
                edges.insert((from.purl.clone(), to.purl.clone(), kind));
            }
        }
    }
    versions.sort();
    let roots = metadata["workspace_members"]
        .as_array()
        .expect("workspace_members")
        .iter()
        .map(|id| rows[id.as_str().expect("member id")].purl.clone())
        .collect();
    SnapshotData { versions, edges, roots }
}

/// Resolves a fixture in a temporary full copy, after an optional edit to
/// that copy. The fixture itself is never touched and nothing is built.
/// Returns the snapshot and the directory, which the caller may inspect.
pub fn resolve_copy(fixture_root: &Path, edit: impl FnOnce(&Path)) -> (SnapshotData, tempfile::TempDir) {
    let temp = tempfile::tempdir().expect("tempdir");
    copy_full(fixture_root, temp.path());
    edit(temp.path());
    let snapshot = resolve_dir(temp.path());
    (snapshot, temp)
}

/// Runs `cargo metadata` in `dir` and reads the snapshot. Never builds.
pub fn resolve_dir(dir: &Path) -> SnapshotData {
    let run = run_cargo(dir, &shared_cargo_home(), &["metadata", "--format-version", "1"]);
    assert!(run.success, "cargo metadata failed: {}", run.stderr);
    let metadata = parse_metadata(&run.stdout, dir);
    let lock = fs::read_to_string(dir.join("Cargo.lock")).expect("read Cargo.lock");
    snapshot_from(&metadata, &lock)
}

// ---- Relate ----------------------------------------------------------------

pub const SCHEMA: [&str; 6] = [
    "CREATE CONSTRAINT project_name FOR (p:Project) REQUIRE p.name IS UNIQUE",
    "CREATE CONSTRAINT snapshot_id FOR (s:Snapshot) REQUIRE s.id IS UNIQUE",
    "CREATE CONSTRAINT package_purl FOR (p:Package) REQUIRE p.purl IS UNIQUE",
    "CREATE CONSTRAINT version_purl FOR (v:Version) REQUIRE v.purl IS UNIQUE",
    "CREATE CONSTRAINT advisory_id FOR (a:Advisory) REQUIRE a.id IS UNIQUE",
    "CREATE CONSTRAINT decision_id FOR (d:Decision) REQUIRE d.id IS UNIQUE",
];

const CREATE_SNAPSHOT: &str = "
MERGE (project:Project {name: $project})
CREATE (project)-[:HAS_SNAPSHOT]->
       (:Snapshot {id: $id, kind: $kind, created_at: $created_at, digest: $digest})";

const LOAD_VERSIONS: &str = "
MATCH (s:Snapshot {id: $snapshot})
UNWIND $versions AS row
MERGE (v:Version {purl: row.purl})
  ON CREATE SET v.source = row.source, v.checksum = row.checksum
MERGE (p:Package {purl: row.package})
MERGE (v)-[:OF]->(p)
MERGE (s)-[:INCLUDES]->(v)";

const LOAD_EDGES: &str = "
UNWIND $edges AS edge
MATCH (a:Version {purl: edge.from})
MATCH (b:Version {purl: edge.to})
CREATE (a)-[:DEPENDS_ON {snapshot: $snapshot, kind: edge.kind}]->(b)";

pub fn text(value: &str) -> Value {
    Value::String(value.into())
}

pub fn params<const N: usize>(pairs: [(&str, Value); N]) -> BTreeMap<String, Value> {
    pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
}

fn map<const N: usize>(pairs: [(&str, Value); N]) -> Value {
    Value::Map(params(pairs).into())
}

pub fn create_schema(conn: &mut Connection) {
    for statement in SCHEMA {
        conn.query(statement).expect("create constraint");
    }
}

/// Loads one snapshot in one transaction and returns the time taken.
pub fn load_snapshot(
    conn: &mut Connection,
    project: &str,
    id: &str,
    kind: &str,
    created_at: &str,
    data: &SnapshotData,
) -> Duration {
    let start = Instant::now();
    conn.begin().expect("begin");
    conn.query_with_params(
        CREATE_SNAPSHOT,
        params([
            ("project", text(project)),
            ("id", text(id)),
            ("kind", text(kind)),
            ("created_at", text(created_at)),
            ("digest", text(&data.digest())),
        ]),
    )
    .expect("create snapshot");

    let versions: Vec<Value> = data
        .versions
        .iter()
        .map(|v| {
            map([
                ("purl", text(&v.purl)),
                ("package", text(&v.package)),
                ("source", text(&v.source)),
                ("checksum", text(&v.checksum)),
            ])
        })
        .collect();
    conn.query_with_params(
        LOAD_VERSIONS,
        params([("snapshot", text(id)), ("versions", Value::List(versions.into()))]),
    )
    .expect("load versions");

    let edges: Vec<Value> = data
        .edges
        .iter()
        .map(|(from, to, kind)| map([("from", text(from)), ("to", text(to)), ("kind", text(kind))]))
        .collect();
    conn.query_with_params(
        LOAD_EDGES,
        params([("snapshot", text(id)), ("edges", Value::List(edges.into()))]),
    )
    .expect("load edges");
    conn.commit().expect("commit snapshot");
    start.elapsed()
}

/// The first column of every row, as strings.
pub fn string_column(rows: &[Vec<Value>]) -> Vec<String> {
    rows.iter()
        .map(|row| row[0].as_string().expect("string column").to_string())
        .collect()
}

/// A list-of-strings cell as a `Vec<String>`.
pub fn string_list(value: &Value) -> Vec<String> {
    match value {
        Value::List(items) => items
            .iter()
            .map(|item| item.as_string().expect("string item").to_string())
            .collect(),
        other => panic!("expected a list, got {other:?}"),
    }
}

/// Total size in bytes of every file under `path`.
pub fn size_on_disk(path: &Path) -> u64 {
    let meta = fs::metadata(path).expect("metadata");
    if meta.is_file() {
        return meta.len();
    }
    fs::read_dir(path)
        .expect("read_dir")
        .map(|entry| size_on_disk(&entry.expect("entry").path()))
        .sum()
}
