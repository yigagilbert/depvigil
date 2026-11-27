//! The plain-text mirror of decisions, `.depvigil/decisions.jsonl`
//! (ADR 0011). One JSON object per line, appended whenever a `Decision` or
//! an `Invalidation` is written. The store remains the source of truth; the
//! mirror exists so that a pull request shows what was decided.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::store::STATE_DIR;
use crate::{Error, Result};

/// The mirror file of a project.
pub fn path_in(project: &Path) -> PathBuf {
    project.join(STATE_DIR).join("decisions.jsonl")
}

/// Appends one record.
pub(crate) fn append(project: &Path, record: &serde_json::Value) -> Result<()> {
    let path = path_in(project);
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| Error::io("open", &path, e))?;
    writeln!(file, "{record}").map_err(|e| Error::io("write", &path, e))
}

/// Reads every record. A project with no mirror yet has none.
pub fn read(project: &Path) -> Result<Vec<serde_json::Value>> {
    let path = path_in(project);
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let text = fs::read_to_string(&path).map_err(|e| Error::io("read", &path, e))?;
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str(line)
                .map_err(|e| Error::parse("decision log", &path, e.to_string()))
        })
        .collect()
}
