//! Advisories, advisory loads and `AFFECTS` relationships.

use serde::Serialize;

use super::{list, map, optional_text, params, text, texts, Params, Row, Store};
use crate::advisory::{Advisory, AdvisoryKind, AffectedPackage};
use crate::{Error, Result};

const COUNT_LOADS: &str = "MATCH (l:AdvisoryLoad) RETURN count(l) AS n";

const CREATE_LOAD: &str = "
CREATE (:AdvisoryLoad {id: $id, seq: $seq, repository: $repository, branch: $branch,
                       commit: $commit, source_date: $source_date, loaded_at: $loaded_at,
                       advisory_count: $advisory_count, published_before: $published_before})";

const LATEST_LOAD: &str = "
MATCH (l:AdvisoryLoad)
RETURN l.id, l.repository, l.branch, l.commit, l.source_date, l.loaded_at,
       l.advisory_count, l.published_before
ORDER BY l.seq DESC LIMIT 1";

const UPSERT_ADVISORIES: &str = "
UNWIND $rows AS row
MERGE (a:Advisory {id: row.id})
SET a.kind = row.kind, a.informational = row.informational, a.summary = row.summary,
    a.published = row.published, a.modified = row.modified, a.withdrawn = row.withdrawn,
    a.packages = row.packages, a.affected = row.affected, a.last_load = $load";

const ADVISORIES_FOR_PACKAGES: &str = "
MATCH (a:Advisory)
WHERE any(name IN a.packages WHERE name IN $names)
RETURN a.id, a.kind, a.informational, a.summary, a.published, a.modified, a.withdrawn, a.affected
ORDER BY a.id";

const ALL_VERSIONS: &str = "
MATCH (v:Version) RETURN v.purl, v.name, v.version, v.source ORDER BY v.purl";

const AFFECTS_OF_ADVISORIES: &str = "
MATCH (a:Advisory)-[:AFFECTS]->(v:Version)
WHERE a.id IN $ids
RETURN a.id, v.purl";

const CREATE_AFFECTS: &str = "
UNWIND $pairs AS pair
MATCH (a:Advisory {id: pair.advisory})
MATCH (v:Version {purl: pair.purl})
MERGE (a)-[r:AFFECTS]->(v)
  ON CREATE SET r.first_seen_at = $now, r.load = $load";

const DELETE_AFFECTS: &str = "
UNWIND $pairs AS pair
MATCH (:Advisory {id: pair.advisory})-[r:AFFECTS]->(:Version {purl: pair.purl})
DELETE r";

const SNAPSHOT_FINDINGS: &str = "
MATCH (:Snapshot {id: $snapshot})-[i:INCLUDES]->(v:Version)<-[r:AFFECTS]-(a:Advisory)
RETURN a.id, a.kind, a.informational, a.summary, a.published, a.withdrawn,
       v.purl, v.name, v.version, i.active, r.first_seen_at
ORDER BY a.id, v.purl";

const COUNT_ADVISORIES: &str = "MATCH (a:Advisory) RETURN count(a) AS n";

/// One recorded advisory load (ADR 0009).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AdvisoryLoadRecord {
    /// The load id, such as `load-0001`.
    pub id: String,
    /// The repository the data came from.
    pub repository: String,
    /// The branch.
    pub branch: String,
    /// The commit.
    pub commit: String,
    /// The commit date of the data.
    pub source_date: String,
    /// When the load ran.
    pub loaded_at: String,
    /// How many advisories the load contained.
    pub advisory_count: i64,
    /// The cut-off used, when only earlier advisories were loaded.
    pub published_before: Option<String>,
}

/// An advisory as findings report it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct StoredAdvisory {
    /// The advisory id.
    pub id: String,
    /// Vulnerability or notice.
    pub kind: AdvisoryKind,
    /// The notice type, such as `unmaintained`.
    pub informational: Option<String>,
    /// A one-line summary.
    pub summary: String,
    /// When it was published.
    pub published: String,
    /// When it was withdrawn, if it was.
    pub withdrawn: Option<String>,
}

/// An advisory affecting a version in a snapshot.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct AffectedVersion {
    /// The advisory.
    pub advisory: StoredAdvisory,
    /// The affected version's Package URL.
    pub purl: String,
    /// The package name.
    pub name: String,
    /// The version.
    pub version: String,
    /// Whether the version is in the resolve graph; `None` when unknown.
    pub active: Option<bool>,
    /// When the store first saw this advisory affect this version.
    pub first_seen_at: String,
}

fn kind(text: &str) -> Result<AdvisoryKind> {
    AdvisoryKind::from_text(text)
        .ok_or_else(|| Error::Corrupt(format!("unknown advisory kind `{text}`")))
}

impl Store {
    /// The id the next advisory load will get, and its sequence number.
    pub(crate) fn next_load(&mut self) -> Result<(String, i64)> {
        let rows = self.query(COUNT_LOADS, Params::new())?;
        let seq = rows
            .first()
            .map(|row| Row(row).int(0))
            .transpose()?
            .unwrap_or(0)
            + 1;
        Ok((format!("load-{seq:04}"), seq))
    }

    /// The most recent advisory load, if any.
    pub fn latest_load(&mut self) -> Result<Option<AdvisoryLoadRecord>> {
        let rows = self.query(LATEST_LOAD, Params::new())?;
        rows.first()
            .map(|row| {
                let row = Row(row);
                Ok(AdvisoryLoadRecord {
                    id: row.string(0)?,
                    repository: row.string(1)?,
                    branch: row.string(2)?,
                    commit: row.string(3)?,
                    source_date: row.string(4)?,
                    loaded_at: row.string(5)?,
                    advisory_count: row.int(6)?,
                    published_before: row.optional_string(7)?,
                })
            })
            .transpose()
    }

    /// Records a load and writes its advisories. An advisory that is already
    /// stored is updated in place, so a later `modified` or `withdrawn` date
    /// takes effect.
    pub(crate) fn store_advisories(
        &mut self,
        load: &AdvisoryLoadRecord,
        seq: i64,
        advisories: &[&Advisory],
    ) -> Result<()> {
        let mut rows = Vec::with_capacity(advisories.len());
        for advisory in advisories {
            let affected =
                serde_json::to_string(&advisory.affected).map_err(|e| Error::Advisory {
                    id: advisory.id.clone(),
                    message: e.to_string(),
                })?;
            rows.push(map([
                ("id", text(&advisory.id)),
                ("kind", text(advisory.kind.as_str())),
                (
                    "informational",
                    optional_text(advisory.informational.as_deref()),
                ),
                ("summary", text(&advisory.summary)),
                ("published", text(&advisory.published)),
                ("modified", text(&advisory.modified)),
                ("withdrawn", optional_text(advisory.withdrawn.as_deref())),
                ("packages", texts(advisory.package_names())),
                ("affected", text(&affected)),
            ]));
        }
        self.query(
            CREATE_LOAD,
            params([
                ("id", text(&load.id)),
                ("seq", relate::Value::Int(seq)),
                ("repository", text(&load.repository)),
                ("branch", text(&load.branch)),
                ("commit", text(&load.commit)),
                ("source_date", text(&load.source_date)),
                ("loaded_at", text(&load.loaded_at)),
                ("advisory_count", relate::Value::Int(load.advisory_count)),
                (
                    "published_before",
                    optional_text(load.published_before.as_deref()),
                ),
            ]),
        )?;
        self.query(
            UPSERT_ADVISORIES,
            params([("rows", list(rows)), ("load", text(&load.id))]),
        )?;
        Ok(())
    }

    /// Stored advisories that name any of these packages.
    pub(crate) fn advisories_for(&mut self, names: &[String]) -> Result<Vec<Advisory>> {
        let rows = self.query(ADVISORIES_FOR_PACKAGES, params([("names", texts(names))]))?;
        let mut advisories = Vec::with_capacity(rows.len());
        for row in &rows {
            let row = Row(row);
            let id = row.string(0)?;
            let affected: Vec<AffectedPackage> = serde_json::from_str(&row.string(7)?)
                .map_err(|e| Error::Corrupt(format!("advisory {id} has unreadable ranges: {e}")))?;
            advisories.push(Advisory {
                kind: kind(&row.string(1)?)?,
                informational: row.optional_string(2)?,
                summary: row.string(3)?,
                published: row.string(4)?,
                modified: row.string(5)?,
                withdrawn: row.optional_string(6)?,
                affected,
                id,
            });
        }
        Ok(advisories)
    }

    /// Every stored version as `(purl, name, version, source)`.
    pub(crate) fn all_versions(&mut self) -> Result<Vec<(String, String, String, String)>> {
        self.query(ALL_VERSIONS, Params::new())?
            .iter()
            .map(|row| {
                let row = Row(row);
                Ok((
                    row.string(0)?,
                    row.string(1)?,
                    row.string(2)?,
                    row.string(3)?,
                ))
            })
            .collect()
    }

    /// The `(advisory, purl)` pairs currently stored for these advisories.
    pub(crate) fn affects_of(&mut self, advisory_ids: &[String]) -> Result<Vec<(String, String)>> {
        self.query(
            AFFECTS_OF_ADVISORIES,
            params([("ids", texts(advisory_ids))]),
        )?
        .iter()
        .map(|row| {
            let row = Row(row);
            Ok((row.string(0)?, row.string(1)?))
        })
        .collect()
    }

    /// Creates `AFFECTS` relationships for the pairs that do not have one
    /// yet, stamped with the time they were first seen and the load that was
    /// current (ADR 0009). Returns how many were new.
    pub(crate) fn add_affects(
        &mut self,
        pairs: &[(String, String)],
        now: &str,
        load: Option<&str>,
    ) -> Result<i64> {
        if pairs.is_empty() {
            return Ok(0);
        }
        let rows = pairs
            .iter()
            .map(|(advisory, purl)| map([("advisory", text(advisory)), ("purl", text(purl))]))
            .collect();
        let result = self.conn.query_with_params(
            CREATE_AFFECTS,
            params([
                ("pairs", list(rows)),
                ("now", text(now)),
                ("load", optional_text(load)),
            ]),
        )?;
        Ok(result.stats.relationships_created)
    }

    /// Removes `AFFECTS` relationships an edited advisory no longer implies.
    pub(crate) fn remove_affects(&mut self, pairs: &[(String, String)]) -> Result<()> {
        if pairs.is_empty() {
            return Ok(());
        }
        let rows = pairs
            .iter()
            .map(|(advisory, purl)| map([("advisory", text(advisory)), ("purl", text(purl))]))
            .collect();
        self.query(DELETE_AFFECTS, params([("pairs", list(rows))]))?;
        Ok(())
    }

    /// Every advisory affecting a version in a snapshot, withdrawn ones
    /// included.
    pub fn findings(&mut self, snapshot: &str) -> Result<Vec<AffectedVersion>> {
        self.query(SNAPSHOT_FINDINGS, params([("snapshot", text(snapshot))]))?
            .iter()
            .map(|row| {
                let row = Row(row);
                Ok(AffectedVersion {
                    advisory: StoredAdvisory {
                        id: row.string(0)?,
                        kind: kind(&row.string(1)?)?,
                        informational: row.optional_string(2)?,
                        summary: row.string(3)?,
                        published: row.string(4)?,
                        withdrawn: row.optional_string(5)?,
                    },
                    purl: row.string(6)?,
                    name: row.string(7)?,
                    version: row.string(8)?,
                    active: row.optional_bool(9)?,
                    first_seen_at: row.string(10)?,
                })
            })
            .collect()
    }

    /// How many advisories the store holds.
    pub fn advisory_count(&mut self) -> Result<i64> {
        let rows = self.query(COUNT_ADVISORIES, Params::new())?;
        rows.first()
            .map(|row| Row(row).int(0))
            .transpose()
            .map(|n| n.unwrap_or(0))
    }
}
