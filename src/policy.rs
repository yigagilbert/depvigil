//! The policy a candidate is judged against, read from `depvigil.toml`
//! (ADR 0010).

use std::fs;
use std::path::Path;

use serde::Serialize;

use crate::{Error, Result};

/// The policy file at the project root.
pub const POLICY_FILE: &str = "depvigil.toml";

/// How vulnerabilities are judged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Reject only when the candidate introduces a vulnerable version the
    /// baseline lacks. Existing vulnerabilities are reported as context.
    #[default]
    Introduced,
    /// Reject when the candidate's dependency set contains any vulnerability.
    Strict,
}

impl Mode {
    /// The name used in `depvigil.toml` and in the store.
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Introduced => "introduced",
            Mode::Strict => "strict",
        }
    }
}

/// The policy for this milestone: vulnerabilities reject, notices warn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct Policy {
    /// How vulnerabilities are judged.
    pub mode: Mode,
}

impl Policy {
    /// Reads `depvigil.toml` from the project root. A missing file means the
    /// default policy.
    pub fn load(project: &Path) -> Result<Policy> {
        let path = project.join(POLICY_FILE);
        if !path.is_file() {
            return Ok(Policy::default());
        }
        let text = fs::read_to_string(&path).map_err(|e| Error::io("read", &path, e))?;
        let table: toml::Table = text
            .parse()
            .map_err(|e: toml::de::Error| Error::parse("policy", &path, e.to_string()))?;
        let mode = match table
            .get("policy")
            .and_then(|p| p.get("mode"))
            .map(|m| m.as_str())
        {
            None => Mode::default(),
            Some(Some("introduced")) => Mode::Introduced,
            Some(Some("strict")) => Mode::Strict,
            Some(other) => {
                return Err(Error::parse(
                    "policy",
                    &path,
                    format!("policy.mode must be \"introduced\" or \"strict\", found {other:?}"),
                ))
            }
        };
        Ok(Policy { mode })
    }

    /// The key of the `Policy` node a decision links to.
    pub fn id(&self) -> String {
        format!("policy-{}", self.mode.as_str())
    }

    /// The rule in words.
    pub fn rule(&self) -> &'static str {
        match self.mode {
            Mode::Introduced => {
                "Reject a candidate that introduces a vulnerable version the baseline lacks. Notices warn."
            }
            Mode::Strict => "Reject a candidate whose dependency set contains any vulnerability. Notices warn.",
        }
    }
}
