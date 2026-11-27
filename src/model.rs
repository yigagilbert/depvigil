//! Ecosystem-neutral types: package identity, resolved dependency states and
//! the inputs an ecosystem cannot handle.

use std::collections::BTreeSet;
use std::fmt;

use serde::Serialize;
use sha2::{Digest, Sha256};

/// The crates.io registry, as Cargo names it in lockfiles and metadata.
pub const CRATES_IO: &str = "registry+https://github.com/rust-lang/crates.io-index";

/// Where a package comes from. Identity includes the source: the same name
/// and version from two sources are two different packages.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Source {
    /// The ecosystem's default public registry.
    DefaultRegistry,
    /// Another registry, by the ecosystem's own source string.
    Registry(String),
    /// A local path, relative to the workspace root. `.` is the root itself.
    Path(String),
    /// A git repository, by the ecosystem's own source string.
    Git(String),
}

impl Source {
    /// Reads a Cargo source string. `None` means a path dependency, whose
    /// relative directory the caller supplies separately.
    pub fn from_cargo(source: &str) -> Source {
        if source == CRATES_IO {
            Source::DefaultRegistry
        } else if source.starts_with("git+") {
            Source::Git(source.to_string())
        } else {
            Source::Registry(source.to_string())
        }
    }

    /// The canonical text form used in digests and in the store.
    pub fn as_text(&self) -> String {
        match self {
            Source::DefaultRegistry => CRATES_IO.to_string(),
            Source::Registry(source) | Source::Git(source) => source.clone(),
            Source::Path(dir) => format!("path:{dir}"),
        }
    }
}

/// One exact package version from one source.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PackageId {
    /// The package name.
    pub name: String,
    /// The exact version.
    pub version: String,
    /// Where it comes from.
    pub source: Source,
}

impl PackageId {
    /// The Package URL of this version, such as `pkg:cargo/serde@1.0.0`.
    /// Anything that is not from the default registry carries a `source`
    /// qualifier, so identity includes the source.
    pub fn purl(&self, ecosystem: &str) -> String {
        match &self.source {
            Source::DefaultRegistry => format!("pkg:{ecosystem}/{}@{}", self.name, self.version),
            other => format!(
                "pkg:{ecosystem}/{}@{}?source={}",
                self.name,
                self.version,
                other.as_text()
            ),
        }
    }

    /// The Package URL of the package without a version, such as
    /// `pkg:cargo/serde`.
    pub fn package_purl(&self, ecosystem: &str) -> String {
        format!("pkg:{ecosystem}/{}", self.name)
    }
}

/// One package in a resolved state.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ResolvedPackage {
    /// The package identity.
    pub id: PackageId,
    /// The Package URL, precomputed.
    pub purl: String,
    /// The lockfile checksum. `None` for path dependencies.
    pub checksum: Option<String>,
    /// Whether the package is part of the resolve graph. `None` when the
    /// state was read from the lockfile alone and activity is unknown.
    pub active: Option<bool>,
}

/// The kind of a dependency edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EdgeKind {
    /// An edge read from the lockfile. It has no kind or platform.
    Lock,
    /// A normal dependency in the resolve graph.
    Normal,
    /// A development dependency in the resolve graph.
    Dev,
    /// A build dependency in the resolve graph.
    Build,
}

impl EdgeKind {
    /// The text stored on the edge.
    pub fn as_str(self) -> &'static str {
        match self {
            EdgeKind::Lock => "lock",
            EdgeKind::Normal => "normal",
            EdgeKind::Dev => "dev",
            EdgeKind::Build => "build",
        }
    }
}

/// A dependency edge between two package versions, by Package URL.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DependencyEdge {
    /// The dependent.
    pub from: String,
    /// The dependency.
    pub to: String,
    /// The kind of edge.
    pub kind: EdgeKind,
}

/// How much is known about a resolved state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Precision {
    /// Read from the lockfile: every package and lockfile edge, no activity.
    Lockfile,
    /// Also resolved by the package manager: the resolve graph and activity.
    Full,
}

impl Precision {
    /// The text stored on the snapshot.
    pub fn as_str(self) -> &'static str {
        match self {
            Precision::Lockfile => "lockfile",
            Precision::Full => "full",
        }
    }
}

/// One resolved dependency state of a workspace.
#[derive(Debug, Clone)]
pub struct ResolvedState {
    /// The ecosystem name used in Package URLs, such as `cargo`.
    pub ecosystem: String,
    /// How much is known.
    pub precision: Precision,
    /// Every package in the lockfile, sorted.
    pub packages: Vec<ResolvedPackage>,
    /// Lockfile edges, plus resolve-graph edges at full precision.
    pub edges: BTreeSet<DependencyEdge>,
    /// Package URLs of the workspace members.
    pub roots: Vec<String>,
    /// Canonical lines describing what the workspace declares: each member's
    /// dependencies and features, and workspace-level settings. Supplied by
    /// the ecosystem, already sorted, free of absolute paths.
    pub declared: Vec<String>,
}

impl ResolvedState {
    /// The canonical text bound by an approval (ADR 0003): the package list,
    /// the lockfile edges and the declared dependencies. It is the same for
    /// a lockfile-precision and a full-precision view of one workspace.
    pub fn canonical(&self) -> String {
        let mut text = String::from("depvigil canonical state v1\n");
        for package in &self.packages {
            text.push_str(&format!(
                "package {} {} {} {}\n",
                package.id.name,
                package.id.version,
                package.id.source.as_text(),
                package.checksum.as_deref().unwrap_or("-")
            ));
        }
        for edge in self.edges.iter().filter(|edge| edge.kind == EdgeKind::Lock) {
            text.push_str(&format!("edge {} -> {}\n", edge.from, edge.to));
        }
        for line in &self.declared {
            text.push_str(line);
            text.push('\n');
        }
        text
    }

    /// The SHA-256 of [`ResolvedState::canonical`], in lowercase hex.
    pub fn digest(&self) -> String {
        sha256_hex(self.canonical().as_bytes())
    }

    /// The Package URLs of every package.
    pub fn purls(&self) -> BTreeSet<String> {
        self.packages.iter().map(|p| p.purl.clone()).collect()
    }

    /// The package with this Package URL.
    pub fn package(&self, purl: &str) -> Option<&ResolvedPackage> {
        self.packages.iter().find(|p| p.purl == purl)
    }
}

/// Lowercase hex SHA-256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A change to simulate: adding a dependency or changing its requirement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProposedChange {
    /// The package to add or change.
    pub name: String,
    /// The version requirement, or `None` for the newest.
    pub version: Option<String>,
    /// Features to enable.
    pub features: Vec<String>,
    /// Whether to turn off the default features.
    pub no_default_features: bool,
    /// The workspace member to change, when the workspace has several.
    pub member: Option<String>,
}

impl ProposedChange {
    /// Parses `name` or `name@version`.
    pub fn parse(spec: &str) -> crate::Result<ProposedChange> {
        let (name, version) = match spec.split_once('@') {
            Some((name, version)) => (name, Some(version)),
            None => (spec, None),
        };
        let valid_name = !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        let valid_version = version.is_none_or(|v| {
            !v.is_empty() && !v.starts_with('-') && !v.chars().any(char::is_whitespace)
        });
        if !valid_name || !valid_version {
            return Err(crate::Error::InvalidInput(format!(
                "`{spec}` is not a package name or name@version"
            )));
        }
        Ok(ProposedChange {
            name: name.to_string(),
            version: version.map(str::to_string),
            features: Vec::new(),
            no_default_features: false,
            member: None,
        })
    }

    /// `name` or `name@version`, as the user wrote it.
    pub fn spec(&self) -> String {
        match &self.version {
            Some(version) => format!("{}@{version}", self.name),
            None => self.name.clone(),
        }
    }
}

impl fmt::Display for ProposedChange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "add {}", self.spec())?;
        if let Some(member) = &self.member {
            write!(f, " to {member}")?;
        }
        if !self.features.is_empty() {
            write!(f, " with features {}", self.features.join(","))?;
        }
        if self.no_default_features {
            write!(f, " without default features")?;
        }
        Ok(())
    }
}

/// An input the ecosystem cannot simulate safely (ADR 0008).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UnsupportedInput {
    /// A dependency fetched from a git repository.
    GitDependency {
        /// The manifest that declares it, relative to the workspace root.
        manifest: String,
        /// The dependency name.
        name: String,
    },
    /// A `[patch]` or `[replace]` section.
    PatchSection {
        /// The file that contains it, relative to the workspace root.
        file: String,
    },
    /// A path dependency that lives outside the workspace root.
    PathOutsideRoot {
        /// The manifest that declares it, relative to the workspace root.
        manifest: String,
        /// The dependency name.
        name: String,
        /// The path as written.
        path: String,
    },
}

impl fmt::Display for UnsupportedInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UnsupportedInput::GitDependency { manifest, name } => {
                write!(f, "git dependency `{name}` in {manifest}")
            }
            UnsupportedInput::PatchSection { file } => write!(f, "[patch] section in {file}"),
            UnsupportedInput::PathOutsideRoot {
                manifest,
                name,
                path,
            } => write!(
                f,
                "path dependency `{name}` in {manifest} points outside the workspace root ({path})"
            ),
        }
    }
}
