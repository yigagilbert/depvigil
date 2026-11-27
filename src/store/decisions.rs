//! Decisions and invalidations. A `Decision` node is written once and never
//! modified; its current status is derived from its invalidations (ADR 0004).

use std::collections::BTreeSet;

use serde::Serialize;

use super::{optional_text, params, text, texts, Params, Row, Store};
use crate::advisory::AdvisoryKind;
use crate::{Error, Result};

const COUNT_DECISIONS: &str = "MATCH (d:Decision) RETURN count(d) AS n";

const CREATE_DECISION: &str = "
MATCH (base:Snapshot {id: $baseline})
MERGE (policy:Policy {id: $policy})
  ON CREATE SET policy.rule = $rule
CREATE (d:Decision {id: $id, seq: $seq, verdict: $verdict, decided_at: $decided_at,
                    digest: $digest, change: $change, subject: $subject, mode: $mode,
                    precision: $precision, details: $details})
CREATE (d)-[:COMPARES]->(base)
CREATE (d)-[:UNDER]->(policy)";

const LINK_APPROVES: &str = "
MATCH (d:Decision {id: $id})
MATCH (s:Snapshot {id: $candidate})
CREATE (d)-[:APPROVES]->(s)";

const LINK_REJECTS: &str = "
MATCH (d:Decision {id: $id})
MATCH (s:Snapshot {id: $candidate})
CREATE (d)-[:REJECTS]->(s)";

const LINK_EVIDENCE: &str = "
MATCH (d:Decision {id: $id})
UNWIND $advisories AS advisory_id
MATCH (a:Advisory {id: advisory_id})
CREATE (d)-[:EVIDENCED_BY]->(a)";

const LINK_LOAD: &str = "
MATCH (d:Decision {id: $id})
MATCH (l:AdvisoryLoad {id: $load})
CREATE (d)-[:JUDGED_AGAINST]->(l)";

const LIST_DECISIONS: &str = "
MATCH (d:Decision)-[:COMPARES]->(base:Snapshot)
MATCH (d)-[:APPROVES|REJECTS]->(cand:Snapshot)
OPTIONAL MATCH (d)-[:JUDGED_AGAINST]->(l:AdvisoryLoad)
RETURN d.id, d.verdict, d.decided_at, d.digest, d.change, d.subject, d.mode,
       d.precision, d.details, base.id, cand.id, l.id
ORDER BY d.seq";

const LIST_INVALIDATIONS: &str = "
MATCH (i:Invalidation)-[:INVALIDATES]->(d:Decision)
MATCH (i)-[:CAUSED_BY]->(a:Advisory)
RETURN i.id, d.id, a.id, i.purl, i.at, i.reason, a.withdrawn, a.published, a.summary
ORDER BY i.id";

/// Approved decisions affected by an advisory and version pair that the
/// store first saw after the decision (ADR 0009).
const PENDING_INVALIDATIONS: &str = "
MATCH (d:Decision {verdict: 'approved'})-[:APPROVES]->(s:Snapshot)
      -[i:INCLUDES]->(v:Version)<-[r:AFFECTS]-(a:Advisory)
WHERE r.first_seen_at > d.decided_at
  AND a.withdrawn IS NULL
RETURN d.id, s.id, a.id, a.kind, a.informational, a.summary, a.published,
       v.purl, i.active, r.first_seen_at,
       EXISTS { (d)<-[:INVALIDATES]-(:Invalidation)-[:CAUSED_BY]->(a) } AS recorded
ORDER BY d.id, a.id, v.purl";

const RECORD_INVALIDATION: &str = "
MATCH (d:Decision {id: $decision})
MATCH (a:Advisory {id: $advisory})
MERGE (i:Invalidation {id: $id})
  ON CREATE SET i.at = $at, i.reason = $reason, i.purl = $purl
MERGE (i)-[:INVALIDATES]->(d)
MERGE (i)-[:CAUSED_BY]->(a)";

const RAW_DECISION: &str = "MATCH (d:Decision {id: $id}) RETURN d";

const MENTIONS_BY_SUBJECT: &str = "MATCH (d:Decision {subject: $name}) RETURN d.id";

/// Decisions whose candidate brought in a version of the package that the
/// baseline it was compared with did not have.
const MENTIONS_BY_INTRODUCTION: &str = "
MATCH (d:Decision)-[:APPROVES|REJECTS]->(:Snapshot)-[:INCLUDES]->(v:Version)-[:OF]->(:Package {name: $name})
MATCH (d)-[:COMPARES]->(base:Snapshot)
WHERE NOT EXISTS { (base)-[:INCLUDES]->(v) }
RETURN DISTINCT d.id";

const MENTIONS_BY_INVALIDATION: &str = "
MATCH (i:Invalidation)-[:INVALIDATES]->(d:Decision)
MATCH (v:Version {purl: i.purl})-[:OF]->(:Package {name: $name})
RETURN DISTINCT d.id";

/// A decision to write.
#[derive(Debug, Clone)]
pub struct NewDecision {
    /// `approved` or `rejected`.
    pub verdict: String,
    /// When the decision was made.
    pub decided_at: String,
    /// The digest of the candidate state.
    pub digest: String,
    /// The change, as text.
    pub change: String,
    /// The package the change is about.
    pub subject: String,
    /// The policy mode in force.
    pub mode: String,
    /// `lockfile` or `full`: which simulation step decided.
    pub precision: String,
    /// The findings behind the verdict, as JSON.
    pub details: String,
    /// The baseline snapshot the candidate was compared with.
    pub baseline: String,
    /// The candidate snapshot.
    pub candidate: String,
    /// The advisories behind a rejection or warning.
    pub evidence: Vec<String>,
    /// The advisory load the decision was judged against, if any was loaded.
    pub load: Option<String>,
}

/// A stored decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DecisionRecord {
    /// The decision id, such as `D0001`.
    pub id: String,
    /// `approved` or `rejected`.
    pub verdict: String,
    /// When it was made.
    pub decided_at: String,
    /// The digest of the candidate state.
    pub digest: String,
    /// The change, as text.
    pub change: String,
    /// The package the change is about.
    pub subject: String,
    /// The policy mode in force.
    pub mode: String,
    /// Which simulation step decided.
    pub precision: String,
    /// The findings behind the verdict, as JSON.
    #[serde(skip)]
    pub details: String,
    /// The baseline snapshot.
    pub baseline: String,
    /// The candidate snapshot.
    pub candidate: String,
    /// The advisory load it was judged against.
    pub load: Option<String>,
    /// Invalidations recorded against it.
    pub invalidations: Vec<InvalidationRecord>,
}

/// A stored invalidation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InvalidationRecord {
    /// The invalidation id.
    pub id: String,
    /// The decision it invalidates.
    pub decision: String,
    /// The advisory that caused it.
    pub advisory: String,
    /// The affected version.
    pub purl: String,
    /// When it was recorded.
    pub at: String,
    /// Why.
    pub reason: String,
    /// When the advisory was withdrawn, if it has been since.
    pub advisory_withdrawn: Option<String>,
    /// When the advisory was published.
    pub advisory_published: String,
    /// The advisory's summary.
    pub advisory_summary: String,
}

/// An approved decision that a newly seen advisory and version pair affects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingInvalidation {
    /// The affected decision.
    pub decision: String,
    /// The snapshot it approved.
    pub snapshot: String,
    /// The advisory id.
    pub advisory: String,
    /// Vulnerability or notice.
    pub kind: AdvisoryKind,
    /// The notice type, if a notice.
    pub informational: Option<String>,
    /// The advisory summary.
    pub summary: String,
    /// When the advisory was published.
    pub published: String,
    /// The affected version.
    pub purl: String,
    /// Whether that version is in the resolve graph; `None` when unknown.
    pub active: Option<bool>,
    /// When the store first saw the advisory affect the version.
    pub first_seen_at: String,
    /// Whether an invalidation for this decision and advisory already exists.
    pub recorded: bool,
}

impl Store {
    /// Writes a decision and returns its id.
    pub fn record_decision(
        &mut self,
        policy_id: &str,
        policy_rule: &str,
        decision: &NewDecision,
    ) -> Result<String> {
        let rows = self.query(COUNT_DECISIONS, Params::new())?;
        let seq = rows
            .first()
            .map(|row| Row(row).int(0))
            .transpose()?
            .unwrap_or(0)
            + 1;
        let id = format!("D{seq:04}");
        let link = match decision.verdict.as_str() {
            "approved" => LINK_APPROVES,
            "rejected" => LINK_REJECTS,
            other => return Err(Error::InvalidInput(format!("unknown verdict `{other}`"))),
        };
        self.transaction(|store| {
            store.query(
                CREATE_DECISION,
                params([
                    ("baseline", text(&decision.baseline)),
                    ("policy", text(policy_id)),
                    ("rule", text(policy_rule)),
                    ("id", text(&id)),
                    ("seq", relate::Value::Int(seq)),
                    ("verdict", text(&decision.verdict)),
                    ("decided_at", text(&decision.decided_at)),
                    ("digest", text(&decision.digest)),
                    ("change", text(&decision.change)),
                    ("subject", text(&decision.subject)),
                    ("mode", text(&decision.mode)),
                    ("precision", text(&decision.precision)),
                    ("details", text(&decision.details)),
                ]),
            )?;
            store.query(
                link,
                params([("id", text(&id)), ("candidate", text(&decision.candidate))]),
            )?;
            if !decision.evidence.is_empty() {
                store.query(
                    LINK_EVIDENCE,
                    params([("id", text(&id)), ("advisories", texts(&decision.evidence))]),
                )?;
            }
            if let Some(load) = &decision.load {
                store.query(
                    LINK_LOAD,
                    params([("id", text(&id)), ("load", optional_text(Some(load)))]),
                )?;
            }
            Ok(())
        })?;
        Ok(id)
    }

    /// Every decision in the order it was made, with its invalidations.
    pub fn decisions(&mut self) -> Result<Vec<DecisionRecord>> {
        let mut decisions = Vec::new();
        for row in &self.query(LIST_DECISIONS, Params::new())? {
            let row = Row(row);
            decisions.push(DecisionRecord {
                id: row.string(0)?,
                verdict: row.string(1)?,
                decided_at: row.string(2)?,
                digest: row.string(3)?,
                change: row.string(4)?,
                subject: row.string(5)?,
                mode: row.string(6)?,
                precision: row.string(7)?,
                details: row.string(8)?,
                baseline: row.string(9)?,
                candidate: row.string(10)?,
                load: row.optional_string(11)?,
                invalidations: Vec::new(),
            });
        }
        for invalidation in self.invalidations()? {
            if let Some(decision) = decisions.iter_mut().find(|d| d.id == invalidation.decision) {
                decision.invalidations.push(invalidation);
            }
        }
        Ok(decisions)
    }

    /// Every invalidation, sorted by id.
    pub fn invalidations(&mut self) -> Result<Vec<InvalidationRecord>> {
        self.query(LIST_INVALIDATIONS, Params::new())?
            .iter()
            .map(|row| {
                let row = Row(row);
                Ok(InvalidationRecord {
                    id: row.string(0)?,
                    decision: row.string(1)?,
                    advisory: row.string(2)?,
                    purl: row.string(3)?,
                    at: row.string(4)?,
                    reason: row.string(5)?,
                    advisory_withdrawn: row.optional_string(6)?,
                    advisory_published: row.string(7)?,
                    advisory_summary: row.string(8)?,
                })
            })
            .collect()
    }

    /// Approved decisions affected by advisory and version pairs first seen
    /// after the decision, whether or not an invalidation exists yet.
    pub fn pending_invalidations(&mut self) -> Result<Vec<PendingInvalidation>> {
        self.query(PENDING_INVALIDATIONS, Params::new())?
            .iter()
            .map(|row| {
                let cells = Row(row);
                let kind_text = cells.string(3)?;
                Ok(PendingInvalidation {
                    decision: cells.string(0)?,
                    snapshot: cells.string(1)?,
                    advisory: cells.string(2)?,
                    kind: AdvisoryKind::from_text(&kind_text).ok_or_else(|| {
                        Error::Corrupt(format!("unknown advisory kind `{kind_text}`"))
                    })?,
                    informational: cells.optional_string(4)?,
                    summary: cells.string(5)?,
                    published: cells.string(6)?,
                    purl: cells.string(7)?,
                    active: cells.optional_bool(8)?,
                    first_seen_at: cells.string(9)?,
                    recorded: cells.optional_bool(10)?.unwrap_or(false),
                })
            })
            .collect()
    }

    /// Records an invalidation. The id is derived from the decision and the
    /// advisory, so recording the same pair again changes nothing. Returns
    /// whether a new node was created.
    pub fn record_invalidation(
        &mut self,
        decision: &str,
        advisory: &str,
        purl: &str,
        at: &str,
        reason: &str,
    ) -> Result<(String, bool)> {
        let id = format!("INV-{decision}-{advisory}");
        let result = self.conn.query_with_params(
            RECORD_INVALIDATION,
            params([
                ("decision", text(decision)),
                ("advisory", text(advisory)),
                ("id", text(&id)),
                ("at", text(at)),
                ("reason", text(reason)),
                ("purl", text(purl)),
            ]),
        )?;
        Ok((id, result.stats.nodes_created > 0))
    }

    /// The `Decision` node exactly as stored, for checking that history is
    /// never rewritten.
    pub fn raw_decision(&mut self, id: &str) -> Result<String> {
        let rows = self.query(RAW_DECISION, params([("id", text(id))]))?;
        rows.first()
            .and_then(|row| row.first())
            .map(|node| format!("{node:?}"))
            .ok_or_else(|| Error::InvalidInput(format!("no decision `{id}`")))
    }

    /// Ids of the decisions that mention a package: it was the subject of
    /// the change, the change introduced a version of it, or an invalidation
    /// names a version of it.
    pub fn decisions_mentioning(&mut self, name: &str) -> Result<BTreeSet<String>> {
        let mut ids = BTreeSet::new();
        for statement in [
            MENTIONS_BY_SUBJECT,
            MENTIONS_BY_INTRODUCTION,
            MENTIONS_BY_INVALIDATION,
        ] {
            for row in &self.query(statement, params([("name", text(name))]))? {
                ids.insert(Row(row).string(0)?);
            }
        }
        Ok(ids)
    }
}
