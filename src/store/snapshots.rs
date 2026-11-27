//! Snapshots: resolved dependency states, the project baseline, diffs and
//! dependency paths.

use serde::Serialize;

use super::{list, map, optional_text, params, text, texts, Params, Row, Store};
use crate::model::{EdgeKind, Precision, ResolvedState};
use crate::{Error, Result};

const FIND_SNAPSHOT: &str = "
MATCH (s:Snapshot {id: $id})
RETURN s.id, s.kind, s.digest, s.precision, s.canonical, s.created_at";

const CREATE_SNAPSHOT: &str = "
MERGE (project:Project {name: $project})
CREATE (s:Snapshot {id: $id, kind: $kind, created_at: $now, digest: $digest,
                    precision: $precision, canonical: $canonical})
CREATE (project)-[:HAS_SNAPSHOT]->(s)";

const SET_PRECISION: &str = "MATCH (s:Snapshot {id: $id}) SET s.precision = $precision";

const LOAD_VERSIONS: &str = "
MATCH (s:Snapshot {id: $snapshot})
UNWIND $versions AS row
MERGE (v:Version {purl: row.purl})
  ON CREATE SET v.name = row.name, v.version = row.version,
                v.source = row.source, v.checksum = row.checksum
MERGE (p:Package {purl: row.package})
  ON CREATE SET p.name = row.name
MERGE (v)-[:OF]->(p)
MERGE (s)-[i:INCLUDES]->(v)
SET i.active = row.active";

const LOAD_EDGES: &str = "
UNWIND $edges AS edge
MATCH (a:Version {purl: edge.from})
MATCH (b:Version {purl: edge.to})
MERGE (a)-[:DEPENDS_ON {snapshot: $snapshot, kind: edge.kind}]->(b)";

const LOAD_ROOTS: &str = "
MATCH (s:Snapshot {id: $snapshot})
UNWIND $roots AS purl
MATCH (v:Version {purl: purl})
MERGE (s)-[:HAS_ROOT]->(v)";

const CLEAR_BASELINE: &str = "MATCH (:Project {name: $project})-[b:BASELINE]->(:Snapshot) DELETE b";

const SET_BASELINE: &str = "
MATCH (project:Project {name: $project})
MATCH (s:Snapshot {id: $id})
CREATE (project)-[:BASELINE {since: $now}]->(s)";

const FIND_BASELINE: &str = "
MATCH (:Project {name: $project})-[:BASELINE]->(s:Snapshot)
RETURN s.id, s.kind, s.digest, s.precision, s.canonical, s.created_at";

const PROJECT_NAME: &str = "MATCH (p:Project) RETURN p.name ORDER BY p.name LIMIT 1";

const SNAPSHOT_STATS: &str = "
MATCH (s:Snapshot {id: $id})-[i:INCLUDES]->(:Version)
RETURN count(i) AS packages,
       sum(CASE WHEN i.active = true THEN 1 ELSE 0 END) AS active,
       sum(CASE WHEN i.active = false THEN 1 ELSE 0 END) AS inactive";

const COUNT_EDGES: &str = "
MATCH (:Version)-[r:DEPENDS_ON {snapshot: $id}]->(:Version)
RETURN sum(CASE WHEN r.kind = 'lock' THEN 1 ELSE 0 END) AS lock_edges,
       sum(CASE WHEN r.kind = 'lock' THEN 0 ELSE 1 END) AS resolve_edges";

const COUNT_ROOTS: &str =
    "MATCH (:Snapshot {id: $id})-[r:HAS_ROOT]->(:Version) RETURN count(r) AS n";

/// Versions in the candidate that are not in the baseline (Phase 0 Test 2).
const DIFF_ADDED: &str = "
MATCH (:Snapshot {id: $cand})-[:INCLUDES]->(v:Version)
WHERE NOT EXISTS { (:Snapshot {id: $base})-[:INCLUDES]->(v) }
RETURN v.purl AS purl ORDER BY purl";

/// Versions in the baseline that are not in the candidate.
const DIFF_REMOVED: &str = "
MATCH (:Snapshot {id: $base})-[:INCLUDES]->(v:Version)
WHERE NOT EXISTS { (:Snapshot {id: $cand})-[:INCLUDES]->(v) }
RETURN v.purl AS purl ORDER BY purl";

/// Packages present in both snapshots at different versions.
const DIFF_CHANGED: &str = "
MATCH (:Snapshot {id: $base})-[:INCLUDES]->(old:Version)-[:OF]->(p:Package),
      (:Snapshot {id: $cand})-[:INCLUDES]->(new:Version)-[:OF]->(p)
WHERE NOT EXISTS { (:Snapshot {id: $cand})-[:INCLUDES]->(old) }
  AND NOT EXISTS { (:Snapshot {id: $base})-[:INCLUDES]->(new) }
RETURN old.purl + ' -> ' + new.purl AS change ORDER BY change";

/// One shortest dependency path from each workspace member to a version,
/// along edges of the given kinds within one snapshot.
const SHORTEST_PATHS: &str = "
MATCH (s:Snapshot {id: $snapshot})-[:HAS_ROOT]->(root:Version)
MATCH p = SHORTEST 1 (root)
          (()-[r:DEPENDS_ON WHERE r.snapshot = $snapshot AND r.kind IN $kinds]->()){1,64}
          (target:Version {purl: $purl})
RETURN [n IN nodes(p) | n.purl] AS purls
ORDER BY size(purls), purls[0]";

const IS_ROOT: &str = "
MATCH (:Snapshot {id: $snapshot})-[:HAS_ROOT]->(v:Version {purl: $purl}) RETURN count(v) AS n";

const VERSIONS_OF_PACKAGE: &str = "
MATCH (:Snapshot {id: $snapshot})-[i:INCLUDES]->(v:Version)-[:OF]->(:Package {name: $name})
RETURN v.purl AS purl, i.active AS active ORDER BY purl";

/// A stored snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SnapshotRecord {
    /// The snapshot id, derived from its digest.
    pub id: String,
    /// `baseline`, `candidate` or `observed`: the role it was first stored in.
    pub kind: String,
    /// The digest of its canonical text.
    pub digest: String,
    /// `lockfile` or `full`.
    pub precision: String,
    /// The canonical text the digest was computed from.
    #[serde(skip)]
    pub canonical: String,
    /// When it was first stored.
    pub created_at: String,
}

/// Counts for one snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SnapshotStats {
    /// Every package in the lockfile.
    pub packages: i64,
    /// Packages in the resolve graph.
    pub active: i64,
    /// Lockfile-only packages.
    pub inactive: i64,
    /// Edges of the resolve graph.
    pub resolve_edges: i64,
    /// Edges read from the lockfile.
    pub lock_edges: i64,
    /// Workspace members.
    pub roots: i64,
}

/// How a candidate snapshot differs from a baseline, by Package URL.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SnapshotDiff {
    /// Versions only in the candidate.
    pub added: Vec<String>,
    /// Versions only in the baseline.
    pub removed: Vec<String>,
    /// `old -> new` for packages whose version changed.
    pub changed: Vec<String>,
}

fn record(row: &[relate::Value]) -> Result<SnapshotRecord> {
    let row = Row(row);
    Ok(SnapshotRecord {
        id: row.string(0)?,
        kind: row.string(1)?,
        digest: row.string(2)?,
        precision: row.string(3)?,
        canonical: row.string(4)?,
        created_at: row.string(5)?,
    })
}

/// The id of the snapshot with this digest.
pub(crate) fn snapshot_id(digest: &str) -> String {
    format!("snap-{}", &digest[..digest.len().min(16)])
}

impl Store {
    /// The snapshot with this id, if stored.
    pub fn snapshot(&mut self, id: &str) -> Result<Option<SnapshotRecord>> {
        let rows = self.query(FIND_SNAPSHOT, params([("id", text(id))]))?;
        rows.first().map(|row| record(row)).transpose()
    }

    /// Stores a resolved state and returns its snapshot id.
    ///
    /// A snapshot is identified by its digest, so storing the same state
    /// twice adds nothing. Storing a full-precision view of a snapshot that
    /// was first stored from the lockfile adds the resolve graph to it.
    pub fn save_snapshot(
        &mut self,
        project: &str,
        state: &ResolvedState,
        kind: &str,
        now: &str,
    ) -> Result<String> {
        let digest = state.digest();
        let id = snapshot_id(&digest);
        let existing = self.snapshot(&id)?;
        let needs_upgrade = match &existing {
            None => false,
            Some(found) => {
                found.precision == Precision::Lockfile.as_str()
                    && state.precision == Precision::Full
            }
        };
        if existing.is_some() && !needs_upgrade {
            return Ok(id);
        }

        let versions: Vec<_> = state
            .packages
            .iter()
            .map(|package| {
                map([
                    ("purl", text(&package.purl)),
                    ("package", text(&package.id.package_purl(&state.ecosystem))),
                    ("name", text(&package.id.name)),
                    ("version", text(&package.id.version)),
                    ("source", text(&package.id.source.as_text())),
                    ("checksum", optional_text(package.checksum.as_deref())),
                    (
                        "active",
                        package
                            .active
                            .map(relate::Value::Bool)
                            .unwrap_or(relate::Value::Null),
                    ),
                ])
            })
            .collect();
        let edges: Vec<_> = state
            .edges
            .iter()
            // The lockfile edges are already there when upgrading.
            .filter(|edge| !(needs_upgrade && edge.kind == EdgeKind::Lock))
            .map(|edge| {
                map([
                    ("from", text(&edge.from)),
                    ("to", text(&edge.to)),
                    ("kind", text(edge.kind.as_str())),
                ])
            })
            .collect();
        let canonical = state.canonical();
        let is_new = existing.is_none();

        self.transaction(|store| {
            if is_new {
                store.query(
                    CREATE_SNAPSHOT,
                    params([
                        ("project", text(project)),
                        ("id", text(&id)),
                        ("kind", text(kind)),
                        ("now", text(now)),
                        ("digest", text(&digest)),
                        ("precision", text(state.precision.as_str())),
                        ("canonical", text(&canonical)),
                    ]),
                )?;
            } else {
                store.query(
                    SET_PRECISION,
                    params([
                        ("id", text(&id)),
                        ("precision", text(state.precision.as_str())),
                    ]),
                )?;
            }
            store.query(
                LOAD_VERSIONS,
                params([("snapshot", text(&id)), ("versions", list(versions))]),
            )?;
            store.query(
                LOAD_EDGES,
                params([("snapshot", text(&id)), ("edges", list(edges))]),
            )?;
            store.query(
                LOAD_ROOTS,
                params([("snapshot", text(&id)), ("roots", texts(&state.roots))]),
            )?;
            Ok(())
        })?;
        Ok(id)
    }

    /// The name of the project this store belongs to, once `init` has run.
    /// A store holds one project, and its name does not depend on the
    /// directory the project happens to be checked out in.
    pub fn project_name(&mut self) -> Result<Option<String>> {
        let rows = self.query(PROJECT_NAME, Params::new())?;
        rows.first().map(|row| Row(row).string(0)).transpose()
    }

    /// The project's current baseline snapshot.
    pub fn baseline(&mut self, project: &str) -> Result<Option<SnapshotRecord>> {
        let rows = self.query(FIND_BASELINE, params([("project", text(project))]))?;
        rows.first().map(|row| record(row)).transpose()
    }

    /// Makes a stored snapshot the project's baseline.
    pub fn set_baseline(&mut self, project: &str, id: &str, now: &str) -> Result<()> {
        self.transaction(|store| {
            store.query(CLEAR_BASELINE, params([("project", text(project))]))?;
            store.query(
                SET_BASELINE,
                params([
                    ("project", text(project)),
                    ("id", text(id)),
                    ("now", text(now)),
                ]),
            )?;
            Ok(())
        })
    }

    /// Package and edge counts of a snapshot.
    pub fn snapshot_stats(&mut self, id: &str) -> Result<SnapshotStats> {
        let single = |rows: Vec<Vec<relate::Value>>| {
            rows.into_iter()
                .next()
                .ok_or_else(|| Error::Corrupt("a count query returned no row".to_string()))
        };
        // `sum` over no rows is null, so read missing counts as zero.
        let count = |row: &[relate::Value], index: usize| match row.get(index) {
            Some(relate::Value::Int(n)) => *n,
            _ => 0,
        };
        let packages = single(self.query(SNAPSHOT_STATS, params([("id", text(id))]))?)?;
        let edges = single(self.query(COUNT_EDGES, params([("id", text(id))]))?)?;
        let roots = single(self.query(COUNT_ROOTS, params([("id", text(id))]))?)?;
        Ok(SnapshotStats {
            packages: count(&packages, 0),
            active: count(&packages, 1),
            inactive: count(&packages, 2),
            lock_edges: count(&edges, 0),
            resolve_edges: count(&edges, 1),
            roots: count(&roots, 0),
        })
    }

    /// How `candidate` differs from `baseline`.
    pub fn diff(&mut self, baseline: &str, candidate: &str) -> Result<SnapshotDiff> {
        let mut column = |statement: &'static str| -> Result<Vec<String>> {
            let parameters: Params = params([("base", text(baseline)), ("cand", text(candidate))]);
            self.query(statement, parameters)?
                .iter()
                .map(|row| Row(row).string(0))
                .collect()
        };
        Ok(SnapshotDiff {
            added: column(DIFF_ADDED)?,
            removed: column(DIFF_REMOVED)?,
            changed: column(DIFF_CHANGED)?,
        })
    }

    /// The shortest dependency path from a workspace member to `purl` within
    /// a snapshot, as Package URLs from the member to the target. Resolve
    /// graph edges are used when the snapshot has them, lockfile edges
    /// otherwise. `None` when no member reaches the version.
    pub fn shortest_path(&mut self, snapshot: &str, purl: &str) -> Result<Option<Vec<String>>> {
        let is_root = self.query(
            IS_ROOT,
            params([("snapshot", text(snapshot)), ("purl", text(purl))]),
        )?;
        if is_root.first().map(|row| Row(row).int(0)).transpose()? == Some(1) {
            return Ok(Some(vec![purl.to_string()]));
        }
        for kinds in [vec!["normal", "build", "dev"], vec!["lock"]] {
            let rows = self.query(
                SHORTEST_PATHS,
                params([
                    ("snapshot", text(snapshot)),
                    ("purl", text(purl)),
                    ("kinds", texts(kinds)),
                ]),
            )?;
            if let Some(row) = rows.first() {
                return Ok(Some(Row(row).strings(0)?));
            }
        }
        Ok(None)
    }

    /// Every version of a package in a snapshot, with its activity.
    pub fn versions_of(
        &mut self,
        snapshot: &str,
        name: &str,
    ) -> Result<Vec<(String, Option<bool>)>> {
        self.query(
            VERSIONS_OF_PACKAGE,
            params([("snapshot", text(snapshot)), ("name", text(name))]),
        )?
        .iter()
        .map(|row| {
            let row = Row(row);
            Ok((row.string(0)?, row.optional_bool(1)?))
        })
        .collect()
    }
}
