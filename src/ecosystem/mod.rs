//! The boundary between the ecosystem-neutral engine and a package manager.

use std::path::Path;

use crate::model::{Precision, ProposedChange, ResolvedState, UnsupportedInput};
use crate::Result;

pub mod cargo;

/// A proposed change resolved in isolation. It owns the temporary copy the
/// change was applied to, so the same copy can be refined later.
#[derive(Debug)]
pub struct Candidate {
    /// The change that was applied.
    pub change: ProposedChange,
    /// The resolved state after the change.
    pub state: ResolvedState,
    pub(crate) workdir: tempfile::TempDir,
}

impl Candidate {
    /// The temporary directory holding the isolated copy. It is removed when
    /// the candidate is dropped.
    pub fn workdir(&self) -> &Path {
        self.workdir.path()
    }
}

/// What the engine needs from a package manager.
///
/// An implementation must never build, test or run third-party code, and
/// must never change the project it is pointed at.
pub trait Ecosystem {
    /// The ecosystem name used in Package URLs, such as `cargo`.
    fn name(&self) -> &'static str;

    /// A short, path-free name for the project, used as the `Project` key.
    fn project_name(&self, project: &Path) -> Result<String>;

    /// Inputs in the project that cannot be simulated safely. An empty list
    /// means the project is supported.
    fn unsupported_inputs(&self, project: &Path) -> Result<Vec<UnsupportedInput>>;

    /// Reads the current resolved state of the project. Lockfile precision
    /// reads files only. Full precision resolves in an isolated copy.
    fn current_state(&self, project: &Path, precision: Precision) -> Result<ResolvedState>;

    /// Applies `change` to an isolated copy of the project and returns the
    /// resulting state at lockfile precision.
    fn resolve_change(&self, project: &Path, change: &ProposedChange) -> Result<Candidate>;

    /// Upgrades a candidate to full precision, in the same isolated copy.
    fn refine(&self, candidate: &mut Candidate) -> Result<()>;

    /// The package-manager commands run so far, for transparency and tests.
    fn tool_invocations(&self) -> Vec<String>;
}
