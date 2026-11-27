//! The Relate directory store (ADR 0001).
//!
//! Every Cypher statement in this module tree is a constant. Values travel
//! only as parameters; no statement is ever built by string concatenation.
//!
//! Schema, as amended by the decision records:
//!
//! ```text
//! (Project)-[:HAS_SNAPSHOT]->(Snapshot)      (Project)-[:BASELINE]->(Snapshot)
//! (Snapshot)-[:INCLUDES {active}]->(Version) (Snapshot)-[:HAS_ROOT]->(Version)
//! (Version)-[:OF]->(Package)
//! (Version)-[:DEPENDS_ON {snapshot, kind}]->(Version)
//! (Advisory)-[:AFFECTS {first_seen_at, load}]->(Version)
//! (Decision)-[:COMPARES]->(Snapshot)         (Decision)-[:APPROVES|REJECTS]->(Snapshot)
//! (Decision)-[:UNDER]->(Policy)              (Decision)-[:EVIDENCED_BY]->(Advisory)
//! (Decision)-[:JUDGED_AGAINST]->(AdvisoryLoad)
//! (Invalidation)-[:INVALIDATES]->(Decision)  (Invalidation)-[:CAUSED_BY]->(Advisory)
//! ```

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use relate::{Connection, Database, Value};

use crate::{Error, Result};

mod advisories;
mod decisions;
mod snapshots;

pub use advisories::{AdvisoryLoadRecord, AffectedVersion, StoredAdvisory};
pub use decisions::{DecisionRecord, InvalidationRecord, NewDecision, PendingInvalidation};
pub use snapshots::{SnapshotDiff, SnapshotRecord, SnapshotStats};

/// The directory depvigil keeps inside a project.
pub const STATE_DIR: &str = ".depvigil";

const SCHEMA: [&str; 9] = [
    "CREATE CONSTRAINT project_name FOR (n:Project) REQUIRE n.name IS UNIQUE",
    "CREATE CONSTRAINT snapshot_id FOR (n:Snapshot) REQUIRE n.id IS UNIQUE",
    "CREATE CONSTRAINT package_purl FOR (n:Package) REQUIRE n.purl IS UNIQUE",
    "CREATE CONSTRAINT version_purl FOR (n:Version) REQUIRE n.purl IS UNIQUE",
    "CREATE CONSTRAINT advisory_id FOR (n:Advisory) REQUIRE n.id IS UNIQUE",
    "CREATE CONSTRAINT advisory_load_id FOR (n:AdvisoryLoad) REQUIRE n.id IS UNIQUE",
    "CREATE CONSTRAINT policy_id FOR (n:Policy) REQUIRE n.id IS UNIQUE",
    "CREATE CONSTRAINT decision_id FOR (n:Decision) REQUIRE n.id IS UNIQUE",
    "CREATE CONSTRAINT invalidation_id FOR (n:Invalidation) REQUIRE n.id IS UNIQUE",
];

const COUNT_NODES: &str = "MATCH (n) RETURN labels(n)[0] AS label, count(n) AS n ORDER BY label";
const COUNT_RELATIONSHIPS: &str =
    "MATCH ()-[r]->() RETURN type(r) AS type, count(r) AS n ORDER BY type";

pub(crate) type Params = BTreeMap<String, Value>;

pub(crate) fn text(value: &str) -> Value {
    Value::String(value.into())
}

pub(crate) fn optional_text(value: Option<&str>) -> Value {
    value.map(text).unwrap_or(Value::Null)
}

pub(crate) fn list(values: Vec<Value>) -> Value {
    Value::List(values.into())
}

pub(crate) fn texts<S: AsRef<str>>(values: impl IntoIterator<Item = S>) -> Value {
    list(values.into_iter().map(|v| text(v.as_ref())).collect())
}

pub(crate) fn params<const N: usize>(pairs: [(&str, Value); N]) -> Params {
    pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
}

pub(crate) fn map<const N: usize>(pairs: [(&str, Value); N]) -> Value {
    Value::Map(params(pairs).into())
}

/// Typed access to one result row. A cell of the wrong type means the store
/// holds something the engine did not write, reported as [`Error::Corrupt`].
pub(crate) struct Row<'a>(pub &'a [Value]);

impl Row<'_> {
    fn cell(&self, index: usize) -> Result<&Value> {
        self.0
            .get(index)
            .ok_or_else(|| Error::Corrupt(format!("result row has no column {index}")))
    }

    pub fn string(&self, index: usize) -> Result<String> {
        match self.cell(index)? {
            Value::String(value) => Ok(value.to_string()),
            other => Err(Error::Corrupt(format!(
                "expected text in column {index}, found {other:?}"
            ))),
        }
    }

    pub fn optional_string(&self, index: usize) -> Result<Option<String>> {
        match self.cell(index)? {
            Value::Null => Ok(None),
            Value::String(value) => Ok(Some(value.to_string())),
            other => Err(Error::Corrupt(format!(
                "expected text in column {index}, found {other:?}"
            ))),
        }
    }

    pub fn int(&self, index: usize) -> Result<i64> {
        match self.cell(index)? {
            Value::Int(value) => Ok(*value),
            other => Err(Error::Corrupt(format!(
                "expected a number in column {index}, found {other:?}"
            ))),
        }
    }

    pub fn optional_bool(&self, index: usize) -> Result<Option<bool>> {
        match self.cell(index)? {
            Value::Null => Ok(None),
            Value::Bool(value) => Ok(Some(*value)),
            other => Err(Error::Corrupt(format!(
                "expected a flag in column {index}, found {other:?}"
            ))),
        }
    }

    pub fn strings(&self, index: usize) -> Result<Vec<String>> {
        match self.cell(index)? {
            Value::Null => Ok(Vec::new()),
            Value::List(items) => items
                .iter()
                .map(|item| match item {
                    Value::String(value) => Ok(value.to_string()),
                    other => Err(Error::Corrupt(format!(
                        "expected text in a list, found {other:?}"
                    ))),
                })
                .collect(),
            other => Err(Error::Corrupt(format!(
                "expected a list in column {index}, found {other:?}"
            ))),
        }
    }
}

/// An open depvigil store.
pub struct Store {
    db: Database,
    conn: Connection,
    path: PathBuf,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store").field("path", &self.path).finish()
    }
}

impl Store {
    /// The store directory of a project.
    pub fn path_in(project: &Path) -> PathBuf {
        project.join(STATE_DIR).join("store")
    }

    fn open_at(path: PathBuf, read_only: bool) -> Result<Store> {
        let uri = path.to_str().ok_or_else(|| {
            Error::InvalidInput(format!(
                "the store path {} is not valid UTF-8",
                path.display()
            ))
        })?;
        let db = if read_only {
            Database::open_read_only(uri)?
        } else {
            Database::open(uri)?
        };
        let conn = db.connect()?;
        Ok(Store { db, conn, path })
    }

    /// Opens the project's store for writing, creating it with its schema
    /// when it does not exist yet.
    pub fn open_or_create(project: &Path) -> Result<Store> {
        let path = Store::path_in(project);
        let existed = path.is_dir();
        if !existed {
            let parent = project.join(STATE_DIR);
            fs::create_dir_all(&parent).map_err(|e| Error::io("create", &parent, e))?;
        }
        let mut store = Store::open_at(path, false)?;
        if !existed {
            for statement in SCHEMA {
                store.conn.query(statement)?;
            }
        }
        Ok(store)
    }

    /// Opens an existing store for writing. Only one process can hold a
    /// store for writing; a second gets [`Error::StoreLocked`].
    pub fn open(project: &Path) -> Result<Store> {
        let path = Store::path_in(project);
        if !path.is_dir() {
            return Err(Error::NotInitialised(project.to_path_buf()));
        }
        Store::open_at(path, false)
    }

    /// Opens an existing store read-only. This works beside a writer.
    pub fn open_read_only(project: &Path) -> Result<Store> {
        let path = Store::path_in(project);
        if !path.is_dir() {
            return Err(Error::NotInitialised(project.to_path_buf()));
        }
        Store::open_at(path, true)
    }

    /// Writes a checkpoint, so the store on disk is a snapshot and not a
    /// long write-ahead log.
    pub fn checkpoint(&self) -> Result<()> {
        Ok(self.db.checkpoint_if_writable()?)
    }

    pub(crate) fn query(
        &mut self,
        statement: &'static str,
        parameters: Params,
    ) -> Result<Vec<Vec<Value>>> {
        Ok(self.conn.query_with_params(statement, parameters)?.rows)
    }

    /// Runs `work` in one transaction: committed if it succeeds, rolled back
    /// if it fails.
    pub(crate) fn transaction<T>(
        &mut self,
        work: impl FnOnce(&mut Store) -> Result<T>,
    ) -> Result<T> {
        self.conn.begin()?;
        match work(self) {
            Ok(value) => {
                self.conn.commit()?;
                Ok(value)
            }
            Err(error) => {
                // The original error is the one worth reporting.
                let _ = self.conn.rollback();
                Err(error)
            }
        }
    }

    /// The number of nodes per label and relationships per type. Used to
    /// show that a repeated command adds nothing.
    pub fn counts(&mut self) -> Result<BTreeMap<String, i64>> {
        let mut counts = BTreeMap::new();
        for row in self.query(COUNT_NODES, Params::new())? {
            let row = Row(&row);
            counts.insert(format!("node:{}", row.string(0)?), row.int(1)?);
        }
        for row in self.query(COUNT_RELATIONSHIPS, Params::new())? {
            let row = Row(&row);
            counts.insert(format!("relationship:{}", row.string(0)?), row.int(1)?);
        }
        Ok(counts)
    }
}
