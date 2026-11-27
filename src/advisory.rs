//! OSV advisories: parsing, version-range evaluation and reading a snapshot
//! from disk. The OSV export is the only advisory source (ADR 0007).

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::time::normalise;
use crate::{Error, Result};

/// The ecosystem name OSV uses for crates.io packages.
const OSV_CRATES_IO: &str = "crates.io";

/// What an advisory means for policy (ADR 0004).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AdvisoryKind {
    /// A security vulnerability. It rejects a candidate and invalidates an
    /// approval.
    Vulnerability,
    /// An informational notice, such as an unmaintained crate. It warns.
    Notice,
}

impl AdvisoryKind {
    /// The text stored on the advisory.
    pub fn as_str(self) -> &'static str {
        match self {
            AdvisoryKind::Vulnerability => "vulnerability",
            AdvisoryKind::Notice => "notice",
        }
    }

    /// Reads the stored text.
    pub fn from_text(text: &str) -> Option<AdvisoryKind> {
        match text {
            "vulnerability" => Some(AdvisoryKind::Vulnerability),
            "notice" => Some(AdvisoryKind::Notice),
            _ => None,
        }
    }
}

/// One event of an OSV `SEMVER` range.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RangeEvent {
    /// Versions from this one on are affected.
    Introduced(String),
    /// Versions from this one on are no longer affected.
    Fixed(String),
}

/// A package an advisory affects, with its version ranges.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AffectedPackage {
    /// The package name on the default registry.
    pub name: String,
    /// Each range is a list of events.
    pub ranges: Vec<Vec<RangeEvent>>,
}

/// One OSV advisory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Advisory {
    /// The advisory id, such as `RUSTSEC-2022-0013`.
    pub id: String,
    /// Vulnerability or notice.
    pub kind: AdvisoryKind,
    /// The notice type the source gave, such as `unmaintained`.
    pub informational: Option<String>,
    /// A one-line summary.
    pub summary: String,
    /// When the advisory was published.
    pub published: String,
    /// When it was last modified.
    pub modified: String,
    /// When it was withdrawn, if it was.
    pub withdrawn: Option<String>,
    /// The packages and version ranges it affects.
    pub affected: Vec<AffectedPackage>,
}

fn version(text: &str, advisory: &str) -> Result<semver::Version> {
    semver::Version::parse(text).map_err(|e| Error::Advisory {
        id: advisory.to_string(),
        message: format!("`{text}` is not a semantic version: {e}"),
    })
}

/// OSV range evaluation: walk the events in version order; an `introduced`
/// at or below the version turns "affected" on, a `fixed` at or below it
/// turns it off.
fn in_range(events: &[RangeEvent], candidate: &semver::Version, advisory: &str) -> Result<bool> {
    let mut parsed = Vec::with_capacity(events.len());
    for event in events {
        parsed.push(match event {
            RangeEvent::Introduced(at) => (version(at, advisory)?, false),
            RangeEvent::Fixed(at) => (version(at, advisory)?, true),
        });
    }
    parsed.sort();
    let mut affected = false;
    for (at, fixed) in parsed {
        if at <= *candidate {
            affected = !fixed;
        }
    }
    Ok(affected)
}

impl Advisory {
    /// Reads one OSV record. Only crates.io packages with `SEMVER` ranges of
    /// `introduced` and `fixed` events are understood; anything else is an
    /// error, never a guess.
    pub fn from_osv(record: &serde_json::Value) -> Result<Advisory> {
        let id = record["id"].as_str().unwrap_or("<no id>").to_string();
        let bad = |message: &str| Error::Advisory {
            id: id.clone(),
            message: message.to_string(),
        };
        let date = |field: &str| -> Result<String> {
            let value = record[field]
                .as_str()
                .ok_or_else(|| bad(&format!("missing `{field}`")))?;
            normalise(value).map_err(|_| bad(&format!("`{field}` is not a UTC timestamp")))
        };
        if record["id"].as_str().is_none() {
            return Err(bad("missing `id`"));
        }
        let withdrawn = match record.get("withdrawn").and_then(|w| w.as_str()) {
            Some(value) => {
                Some(normalise(value).map_err(|_| bad("`withdrawn` is not a UTC timestamp"))?)
            }
            None => None,
        };

        let mut informational = None;
        let mut affected = Vec::new();
        for item in record["affected"]
            .as_array()
            .ok_or_else(|| bad("missing `affected`"))?
        {
            if item["package"]["ecosystem"].as_str() != Some(OSV_CRATES_IO) {
                return Err(bad("an affected package is not from crates.io"));
            }
            let name = item["package"]["name"]
                .as_str()
                .ok_or_else(|| bad("an affected package has no name"))?
                .to_string();
            if let Some(kind) = item["database_specific"]["informational"].as_str() {
                informational.get_or_insert_with(|| kind.to_string());
            }
            let mut ranges = Vec::new();
            for range in item["ranges"].as_array().into_iter().flatten() {
                if range["type"].as_str() != Some("SEMVER") {
                    return Err(bad("only SEMVER ranges are supported"));
                }
                let mut events = Vec::new();
                for event in range["events"]
                    .as_array()
                    .ok_or_else(|| bad("a range has no events"))?
                {
                    let parsed = match (event.get("introduced"), event.get("fixed")) {
                        (Some(at), None) => {
                            at.as_str().map(|at| RangeEvent::Introduced(at.to_string()))
                        }
                        (None, Some(at)) => at.as_str().map(|at| RangeEvent::Fixed(at.to_string())),
                        _ => None,
                    };
                    let parsed = parsed
                        .ok_or_else(|| bad("only `introduced` and `fixed` events are supported"))?;
                    match &parsed {
                        RangeEvent::Introduced(at) | RangeEvent::Fixed(at) => {
                            version(at, &id)?;
                        }
                    }
                    events.push(parsed);
                }
                ranges.push(events);
            }
            affected.push(AffectedPackage { name, ranges });
        }

        Ok(Advisory {
            kind: if informational.is_some() {
                AdvisoryKind::Notice
            } else {
                AdvisoryKind::Vulnerability
            },
            informational,
            summary: record["summary"].as_str().unwrap_or_default().to_string(),
            published: date("published")?,
            modified: date("modified")?,
            withdrawn,
            affected,
            id,
        })
    }

    /// Whether this advisory affects `version` of the default-registry
    /// package `name`. A version that is not a semantic version is never
    /// matched.
    pub fn affects(&self, name: &str, version: &str) -> Result<bool> {
        let Ok(candidate) = semver::Version::parse(version) else {
            return Ok(false);
        };
        for package in self.affected.iter().filter(|p| p.name == name) {
            for range in &package.ranges {
                if in_range(range, &candidate, &self.id)? {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    /// The names of the packages this advisory affects.
    pub fn package_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.affected.iter().map(|p| p.name.clone()).collect();
        names.sort();
        names.dedup();
        names
    }
}

/// Where an advisory snapshot came from.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct AdvisorySource {
    /// The repository the data was taken from.
    pub repository: String,
    /// The branch.
    pub branch: String,
    /// The commit.
    pub commit: String,
    /// The commit date.
    pub date: String,
}

/// A set of advisories read from disk.
#[derive(Debug, Clone)]
pub struct AdvisorySet {
    /// The advisories, sorted by id.
    pub advisories: Vec<Advisory>,
    /// Where they came from, read from a `SOURCE.txt` beside the data.
    pub source: AdvisorySource,
}

fn read_source(dir: &Path) -> AdvisorySource {
    let mut fields = BTreeMap::new();
    if let Ok(text) = fs::read_to_string(dir.join("SOURCE.txt")) {
        for line in text.lines() {
            if let Some((key, value)) = line.split_once(':') {
                fields.insert(key.trim().to_lowercase(), value.trim().to_string());
            }
        }
    }
    let field = |key: &str| {
        fields
            .get(key)
            .cloned()
            .unwrap_or_else(|| "unknown".to_string())
    };
    AdvisorySource {
        repository: field("repository"),
        branch: field("branch"),
        commit: field("commit"),
        date: field("date"),
    }
}

fn parse_record(text: &str, origin: &Path) -> Result<Advisory> {
    let record: serde_json::Value =
        serde_json::from_str(text).map_err(|e| Error::parse("advisory", origin, e.to_string()))?;
    Advisory::from_osv(&record)
}

/// Reads JSON Lines: one OSV record per line.
pub(crate) fn read_json_lines(reader: impl Read, origin: &Path) -> Result<Vec<Advisory>> {
    let mut advisories = Vec::new();
    for line in BufReader::new(reader).lines() {
        let line = line.map_err(|e| Error::io("read", origin, e))?;
        if !line.trim().is_empty() {
            advisories.push(parse_record(&line, origin)?);
        }
    }
    Ok(advisories)
}

/// Reads advisories from `path`: a directory of OSV `.json` files, a JSON
/// Lines file (`.jsonl`), or a gzip-compressed JSON Lines file
/// (`.jsonl.gz`). A `SOURCE.txt` beside the data names where it came from.
pub fn read_advisories(path: &Path) -> Result<AdvisorySet> {
    let (mut advisories, source_dir) = if path.is_dir() {
        let mut advisories = Vec::new();
        for entry in fs::read_dir(path).map_err(|e| Error::io("read", path, e))? {
            let file = entry.map_err(|e| Error::io("read", path, e))?.path();
            if file.extension().and_then(|e| e.to_str()) == Some("json") {
                let text = fs::read_to_string(&file).map_err(|e| Error::io("read", &file, e))?;
                advisories.push(parse_record(&text, &file)?);
            }
        }
        // The source note sits in the directory or beside it.
        let source_dir = if path.join("SOURCE.txt").is_file() {
            path.to_path_buf()
        } else {
            path.parent().unwrap_or(path).to_path_buf()
        };
        (advisories, source_dir)
    } else {
        let file = fs::File::open(path).map_err(|e| Error::io("open", path, e))?;
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        let advisories = if name.ends_with(".jsonl") {
            read_json_lines(file, path)?
        } else if name.ends_with(".jsonl.gz") {
            crate::compressed::read_gzip_json_lines(file, path)?
        } else {
            return Err(Error::InvalidInput(format!(
                "{} is not a directory of OSV .json files, a .jsonl file or a .jsonl.gz file",
                path.display()
            )));
        };
        (
            advisories,
            path.parent().unwrap_or(Path::new(".")).to_path_buf(),
        )
    };
    if advisories.is_empty() {
        return Err(Error::InvalidInput(format!(
            "no advisories found at {}",
            path.display()
        )));
    }
    advisories.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(AdvisorySet {
        advisories,
        source: read_source(&source_dir),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn advisory(events: &[(&str, &str)]) -> Advisory {
        let events: Vec<serde_json::Value> = events
            .iter()
            .map(|(kind, at)| serde_json::json!({ *kind: at }))
            .collect();
        Advisory::from_osv(&serde_json::json!({
            "id": "TEST-0001",
            "published": "2022-03-08T12:00:00Z",
            "modified": "2022-03-09T12:00:00Z",
            "summary": "test",
            "affected": [{
                "package": {"ecosystem": "crates.io", "name": "demo"},
                "database_specific": {"informational": null},
                "ranges": [{"type": "SEMVER", "events": events}]
            }]
        }))
        .expect("valid advisory")
    }

    #[test]
    fn introduced_and_fixed() {
        let a = advisory(&[("introduced", "0.0.0-0"), ("fixed", "1.5.5")]);
        assert_eq!(a.kind, AdvisoryKind::Vulnerability);
        assert!(a.affects("demo", "1.5.4").unwrap());
        assert!(a.affects("demo", "0.1.0").unwrap());
        assert!(!a.affects("demo", "1.5.5").unwrap());
        assert!(!a.affects("demo", "2.0.0").unwrap());
        assert!(!a.affects("other", "1.5.4").unwrap());
    }

    #[test]
    fn several_intervals_in_one_range() {
        let a = advisory(&[
            ("introduced", "1.0.0"),
            ("fixed", "1.2.0"),
            ("introduced", "2.0.0"),
            ("fixed", "2.1.0"),
        ]);
        assert!(!a.affects("demo", "0.9.0").unwrap());
        assert!(a.affects("demo", "1.1.0").unwrap());
        assert!(!a.affects("demo", "1.5.0").unwrap());
        assert!(a.affects("demo", "2.0.5").unwrap());
        assert!(!a.affects("demo", "2.1.0").unwrap());
    }

    #[test]
    fn unpatched_range_affects_everything_after() {
        let a = advisory(&[("introduced", "0.0.0-0")]);
        assert!(a.affects("demo", "99.0.0").unwrap());
    }

    #[test]
    fn unsupported_shapes_are_errors_not_guesses() {
        let record = serde_json::json!({
            "id": "TEST-0002", "published": "2022-03-08T12:00:00Z", "modified": "2022-03-08T12:00:00Z",
            "affected": [{"package": {"ecosystem": "crates.io", "name": "demo"},
                          "ranges": [{"type": "SEMVER", "events": [{"last_affected": "1.0.0"}]}]}]
        });
        assert!(matches!(
            Advisory::from_osv(&record),
            Err(Error::Advisory { .. })
        ));
        let record = serde_json::json!({
            "id": "TEST-0003", "published": "2022-03-08T12:00:00Z", "modified": "2022-03-08T12:00:00Z",
            "affected": [{"package": {"ecosystem": "npm", "name": "demo"}, "ranges": []}]
        });
        assert!(matches!(
            Advisory::from_osv(&record),
            Err(Error::Advisory { .. })
        ));
    }

    #[test]
    fn informational_advisories_are_notices() {
        let record = serde_json::json!({
            "id": "TEST-0004", "published": "2022-03-08T12:00:00Z", "modified": "2022-03-08T12:00:00Z",
            "affected": [{"package": {"ecosystem": "crates.io", "name": "demo"},
                          "database_specific": {"informational": "unmaintained"},
                          "ranges": [{"type": "SEMVER", "events": [{"introduced": "0.0.0-0"}]}]}]
        });
        let a = Advisory::from_osv(&record).unwrap();
        assert_eq!(a.kind, AdvisoryKind::Notice);
        assert_eq!(a.informational.as_deref(), Some("unmaintained"));
    }
}
