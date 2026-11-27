//! The engine: every command as a library function, independent of the
//! command line and of any one package manager.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::advisory::{read_advisories, Advisory, AdvisoryKind};
use crate::ecosystem::Ecosystem;
use crate::mirror;
use crate::model::{Precision, ProposedChange, ResolvedState, Source};
use crate::policy::{Mode, Policy};
use crate::store::{
    AdvisoryLoadRecord, AffectedVersion, DecisionRecord, NewDecision, SnapshotDiff, SnapshotStats,
    Store,
};
use crate::time::{normalise, Clock};
use crate::{Error, Result};

/// The most candidates one `check` compares.
pub const MAX_CANDIDATES: usize = 4;

/// Runs depvigil's commands on one project.
#[derive(Debug)]
pub struct Engine<E: Ecosystem> {
    ecosystem: E,
    project: PathBuf,
    clock: Clock,
}

/// What `init` did.
#[derive(Debug, Clone, Serialize)]
pub struct InitReport {
    /// The project name.
    pub project: String,
    /// The baseline snapshot.
    pub snapshot: String,
    /// Its digest.
    pub digest: String,
    /// Whether this run recorded a new baseline. `false` means the stored
    /// baseline already matched the workspace and nothing was added.
    pub baseline_changed: bool,
    /// Package and edge counts of the baseline.
    pub stats: SnapshotStats,
}

/// What `advisories load` did.
#[derive(Debug, Clone, Serialize)]
pub struct LoadReport {
    /// The recorded load.
    pub load: AdvisoryLoadRecord,
    /// Advisories in the source that the cut-off left out.
    pub skipped_by_cutoff: usize,
    /// Loaded advisories that are vulnerabilities.
    pub vulnerabilities: usize,
    /// Loaded advisories that are notices.
    pub notices: usize,
    /// Loaded advisories that are withdrawn.
    pub withdrawn: usize,
    /// `AFFECTS` relationships seen for the first time.
    pub new_affects: i64,
    /// `AFFECTS` relationships removed because an advisory no longer covers
    /// the version.
    pub removed_affects: usize,
}

/// One advisory affecting one version, as reports show it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FindingReport {
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
    /// The affected version's Package URL.
    pub purl: String,
    /// Whether the version is in the resolve graph; `None` when unknown.
    pub active: Option<bool>,
    /// The shortest dependency path from a workspace member to the version.
    pub path: Option<Vec<String>>,
}

impl FindingReport {
    fn from(finding: &AffectedVersion) -> Self {
        FindingReport {
            advisory: finding.advisory.id.clone(),
            kind: finding.advisory.kind,
            informational: finding.advisory.informational.clone(),
            summary: finding.advisory.summary.clone(),
            published: finding.advisory.published.clone(),
            purl: finding.purl.clone(),
            active: finding.active,
            path: None,
        }
    }
}

/// The verdict on one candidate.
#[derive(Debug, Clone, Serialize)]
pub struct CandidateReport {
    /// The candidate as given, such as `either@1.13.0`.
    pub spec: String,
    /// The change in words.
    pub change: String,
    /// `approved` or `rejected`.
    pub verdict: String,
    /// The decision recorded for it.
    pub decision: String,
    /// The candidate snapshot.
    pub snapshot: String,
    /// The digest an approval is bound to.
    pub digest: String,
    /// Whether the precise step (`cargo metadata`) had to run.
    pub precise_step: bool,
    /// Findings that reject the candidate.
    pub rejections: Vec<FindingReport>,
    /// Findings that only warn: notices, and hits on inactive packages.
    pub warnings: Vec<FindingReport>,
    /// Findings the baseline already had, as context.
    pub existing: Vec<FindingReport>,
    /// Baseline vulnerabilities the candidate removes.
    pub improvements: Vec<FindingReport>,
    /// How the candidate differs from the baseline.
    pub diff: SnapshotDiff,
    /// Whether this is the best acceptable candidate.
    pub best: bool,
}

/// What `check` decided.
#[derive(Debug, Clone, Serialize)]
pub struct CheckReport {
    /// The baseline the candidates were compared with.
    pub baseline: String,
    /// The policy mode in force.
    pub mode: Mode,
    /// The advisory load the decisions were judged against.
    pub advisory_load: Option<String>,
    /// One report per candidate, in the order given.
    pub candidates: Vec<CandidateReport>,
    /// The best acceptable candidate, if any is acceptable.
    pub best: Option<String>,
    /// Why it was ranked best.
    pub best_reason: Option<String>,
    /// Every package-manager command that ran.
    pub commands: Vec<String>,
}

/// One line of difference between the approved state and the workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Difference {
    /// `workspace` when only the workspace has it, `approved` when only the
    /// approved state has it.
    pub only_in: String,
    /// `package`, `edge`, `declared`, `feature` or `resolver`.
    pub kind: String,
    /// The canonical line, without its kind.
    pub detail: String,
}

/// What `verify` found.
#[derive(Debug, Clone, Serialize)]
pub struct VerifyReport {
    /// The latest approved decision.
    pub decision: String,
    /// The change it approved.
    pub change: String,
    /// Its current status.
    pub status: DecisionStatus,
    /// The digest that was approved.
    pub approved_digest: String,
    /// The digest of the workspace now.
    pub current_digest: String,
    /// Whether they are equal.
    pub matches: bool,
    /// What differs, when they are not.
    pub differences: Vec<Difference>,
}

/// The current standing of a decision, derived from its invalidations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DecisionStatus {
    /// A rejection. It has no standing to lose.
    Rejected,
    /// An approval with no invalidation.
    Valid,
    /// An approval with at least one invalidation still in force.
    Invalidated,
    /// An approval whose every invalidating advisory has been withdrawn.
    Restored,
}

impl DecisionStatus {
    /// The status of a stored decision.
    pub fn of(decision: &DecisionRecord) -> DecisionStatus {
        if decision.verdict != "approved" {
            DecisionStatus::Rejected
        } else if decision.invalidations.is_empty() {
            DecisionStatus::Valid
        } else if decision
            .invalidations
            .iter()
            .all(|i| i.advisory_withdrawn.is_some())
        {
            DecisionStatus::Restored
        } else {
            DecisionStatus::Invalidated
        }
    }

    /// The status in words.
    pub fn as_str(self) -> &'static str {
        match self {
            DecisionStatus::Rejected => "rejected",
            DecisionStatus::Valid => "valid",
            DecisionStatus::Invalidated => "invalidated",
            DecisionStatus::Restored => "restored",
        }
    }
}

/// One result of `recheck`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecheckEntry {
    /// The approved decision concerned.
    pub decision: String,
    /// The change it approved.
    pub change: String,
    /// The advisory.
    pub advisory: String,
    /// Vulnerability or notice.
    pub kind: AdvisoryKind,
    /// The advisory summary.
    pub summary: String,
    /// When the advisory was published.
    pub published: String,
    /// When the store first saw it affect the version.
    pub first_seen_at: Option<String>,
    /// The affected version.
    pub purl: String,
    /// The dependency path to it in the approved snapshot.
    pub path: Option<Vec<String>>,
    /// Whether this run recorded the invalidation.
    pub new: bool,
    /// Why a warning is only a warning.
    pub note: Option<String>,
}

/// What `recheck` found.
#[derive(Debug, Clone, Default, Serialize)]
pub struct RecheckReport {
    /// Approved decisions looked at.
    pub approved_decisions: usize,
    /// Approvals invalidated by a vulnerability, new and standing.
    pub invalidated: Vec<RecheckEntry>,
    /// Notices and inactive hits. They invalidate nothing.
    pub warnings: Vec<RecheckEntry>,
    /// Approvals whose only invalidating advisories have been withdrawn.
    pub restored: Vec<RecheckEntry>,
}

/// One decision, as `log` lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LogEntry {
    /// The decision id.
    pub id: String,
    /// `approved` or `rejected`.
    pub verdict: String,
    /// When it was made.
    pub decided_at: String,
    /// Its status now.
    pub status: DecisionStatus,
    /// The change it judged.
    pub change: String,
    /// The digest of the candidate state.
    pub digest: String,
    /// Which simulation step decided.
    pub precision: String,
    /// The advisory load it was judged against.
    pub advisory_load: Option<String>,
    /// Advisories that have invalidated it.
    pub invalidated_by: Vec<String>,
}

impl LogEntry {
    fn from(decision: &DecisionRecord) -> Self {
        LogEntry {
            id: decision.id.clone(),
            verdict: decision.verdict.clone(),
            decided_at: decision.decided_at.clone(),
            status: DecisionStatus::of(decision),
            change: decision.change.clone(),
            digest: decision.digest.clone(),
            precision: decision.precision.clone(),
            advisory_load: decision.load.clone(),
            invalidated_by: decision
                .invalidations
                .iter()
                .map(|i| i.advisory.clone())
                .collect(),
        }
    }
}

/// One version of a package in the baseline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WhyVersion {
    /// The version's Package URL.
    pub purl: String,
    /// Whether it is in the resolve graph.
    pub active: Option<bool>,
    /// The shortest path from a workspace member to it.
    pub path: Option<Vec<String>>,
}

/// What `why` found.
#[derive(Debug, Clone, Serialize)]
pub struct WhyReport {
    /// The package asked about.
    pub package: String,
    /// The baseline snapshot searched.
    pub baseline: String,
    /// Its versions in the baseline, each with a path.
    pub versions: Vec<WhyVersion>,
    /// Past decisions that mention the package.
    pub decisions: Vec<LogEntry>,
}

/// A candidate after simulation, before its decision is recorded.
struct Evaluated {
    change: ProposedChange,
    state_digest: String,
    snapshot: String,
    precise: bool,
    rejections: Vec<FindingReport>,
    warnings: Vec<FindingReport>,
    existing: Vec<FindingReport>,
    improvements: Vec<FindingReport>,
    diff: SnapshotDiff,
    subject_version: Option<semver::Version>,
}

/// What every candidate of one `check` is judged against.
#[derive(Clone, Copy)]
struct CheckContext<'a> {
    project: &'a str,
    policy: Policy,
    baseline: &'a str,
    baseline_findings: &'a [AffectedVersion],
    now: &'a str,
    load: Option<&'a str>,
}

fn finding_key(finding: &AffectedVersion) -> (String, String) {
    (finding.advisory.id.clone(), finding.purl.clone())
}

fn live(findings: Vec<AffectedVersion>) -> Vec<AffectedVersion> {
    findings
        .into_iter()
        .filter(|f| f.advisory.withdrawn.is_none())
        .collect()
}

impl<E: Ecosystem> Engine<E> {
    /// An engine for the project at `project`, using the system clock.
    pub fn new(ecosystem: E, project: impl Into<PathBuf>) -> Self {
        Engine {
            ecosystem,
            project: project.into(),
            clock: Clock::System,
        }
    }

    /// Uses `clock` for every timestamp this engine records.
    pub fn with_clock(mut self, clock: Clock) -> Self {
        self.clock = clock;
        self
    }

    /// The ecosystem this engine drives.
    pub fn ecosystem(&self) -> &E {
        &self.ecosystem
    }

    /// The project directory.
    pub fn project(&self) -> &Path {
        &self.project
    }

    /// The project name: the one the store already holds, or else the one
    /// the ecosystem derives for a new store.
    fn project_name(&self, store: &mut Store) -> Result<String> {
        match store.project_name()? {
            Some(name) => Ok(name),
            None => self.ecosystem.project_name(&self.project),
        }
    }

    /// Creates `AFFECTS` relationships between stored advisories and the
    /// packages of `state` that do not have one yet.
    fn link_advisories(
        &self,
        store: &mut Store,
        state: &ResolvedState,
        now: &str,
        load: Option<&str>,
    ) -> Result<i64> {
        let registry: Vec<_> = state
            .packages
            .iter()
            .filter(|p| p.id.source == Source::DefaultRegistry)
            .collect();
        let names: BTreeSet<String> = registry.iter().map(|p| p.id.name.clone()).collect();
        let advisories = store.advisories_for(&names.into_iter().collect::<Vec<_>>())?;
        let mut pairs = Vec::new();
        for package in registry {
            for advisory in &advisories {
                if advisory.affects(&package.id.name, &package.id.version)? {
                    pairs.push((advisory.id.clone(), package.purl.clone()));
                }
            }
        }
        store.add_affects(&pairs, now, load)
    }

    /// `depvigil init`: creates the store if needed and records the current
    /// workspace as the baseline. Running it again on an unchanged workspace
    /// adds nothing.
    pub fn init(&self) -> Result<InitReport> {
        let state = self
            .ecosystem
            .current_state(&self.project, Precision::Full)?;
        let now = self.clock.now();
        let mut store = Store::open_or_create(&self.project)?;
        let project = self.project_name(&mut store)?;
        let load = store.latest_load()?.map(|l| l.id);
        let snapshot = store.save_snapshot(&project, &state, "baseline", &now)?;
        let baseline_changed = store.baseline()?.map(|b| b.id) != Some(snapshot.clone());
        if baseline_changed {
            store.set_baseline(&project, &snapshot, &now)?;
            self.link_advisories(&mut store, &state, &now, load.as_deref())?;
        }
        let stats = store.snapshot_stats(&snapshot)?;
        store.checkpoint()?;
        Ok(InitReport {
            project,
            snapshot,
            digest: state.digest(),
            baseline_changed,
            stats,
        })
    }

    /// `depvigil advisories load`: loads OSV advisories from `path`, sets
    /// their kind, and records which stored versions they affect. With
    /// `published_before`, only advisories published before that date are
    /// loaded.
    pub fn load_advisories(
        &self,
        path: &Path,
        published_before: Option<&str>,
    ) -> Result<LoadReport> {
        let set = read_advisories(path)?;
        let cutoff = published_before.map(normalise).transpose()?;
        let selected: Vec<&Advisory> = set
            .advisories
            .iter()
            .filter(|a| cutoff.as_ref().is_none_or(|cutoff| a.published < *cutoff))
            .collect();

        let mut store = Store::open(&self.project)?;
        let now = self.clock.now();
        let (id, seq) = store.next_load()?;
        let load = AdvisoryLoadRecord {
            id,
            repository: set.source.repository.clone(),
            branch: set.source.branch.clone(),
            commit: set.source.commit.clone(),
            source_date: set.source.date.clone(),
            loaded_at: now.clone(),
            advisory_count: selected.len() as i64,
            published_before: cutoff.clone(),
        };
        store.transaction(|store| store.store_advisories(&load, seq, &selected))?;

        // Which stored versions do the loaded advisories affect now?
        let mut by_package: BTreeMap<&str, Vec<&Advisory>> = BTreeMap::new();
        for advisory in &selected {
            for package in &advisory.affected {
                by_package
                    .entry(package.name.as_str())
                    .or_default()
                    .push(advisory);
            }
        }
        let mut wanted = BTreeSet::new();
        for (purl, name, version, source) in store.all_versions()? {
            if source != Source::DefaultRegistry.as_text() {
                continue; // identity includes the source
            }
            for advisory in by_package.get(name.as_str()).into_iter().flatten() {
                if advisory.affects(&name, &version)? {
                    wanted.insert((advisory.id.clone(), purl.clone()));
                }
            }
        }
        let ids: Vec<String> = selected.iter().map(|a| a.id.clone()).collect();
        let existing: BTreeSet<(String, String)> = store.affects_of(&ids)?.into_iter().collect();
        let to_add: Vec<_> = wanted.difference(&existing).cloned().collect();
        let to_remove: Vec<_> = existing.difference(&wanted).cloned().collect();
        let new_affects = store.transaction(|store| {
            store.remove_affects(&to_remove)?;
            store.add_affects(&to_add, &now, Some(&load.id))
        })?;
        store.checkpoint()?;

        Ok(LoadReport {
            skipped_by_cutoff: set.advisories.len() - selected.len(),
            vulnerabilities: selected
                .iter()
                .filter(|a| a.kind == AdvisoryKind::Vulnerability)
                .count(),
            notices: selected
                .iter()
                .filter(|a| a.kind == AdvisoryKind::Notice)
                .count(),
            withdrawn: selected.iter().filter(|a| a.withdrawn.is_some()).count(),
            new_affects,
            removed_affects: to_remove.len(),
            load,
        })
    }

    /// Simulates one change with the two-step flow and classifies its
    /// findings against the baseline.
    fn evaluate(
        &self,
        store: &mut Store,
        change: &ProposedChange,
        context: &CheckContext<'_>,
    ) -> Result<Evaluated> {
        let CheckContext {
            project,
            policy,
            baseline,
            baseline_findings,
            now,
            load,
        } = *context;
        // Step 1: the lockfile written by the change, and nothing more.
        let mut candidate = self.ecosystem.resolve_change(&self.project, change)?;
        let snapshot = store.save_snapshot(project, &candidate.state, "candidate", now)?;
        self.link_advisories(store, &candidate.state, now, load)?;
        let mut findings = live(store.findings(&snapshot)?);

        let baseline_keys: BTreeSet<_> = baseline_findings.iter().map(finding_key).collect();
        let counts_against = |finding: &AffectedVersion| {
            !baseline_keys.contains(&finding_key(finding))
                || (policy.mode == Mode::Strict
                    && finding.advisory.kind == AdvisoryKind::Vulnerability)
        };

        // Step 2: only when step 1 found a policy hit.
        let precise = findings.iter().any(counts_against);
        if precise {
            self.ecosystem.refine(&mut candidate)?;
            store.save_snapshot(project, &candidate.state, "candidate", now)?;
            findings = live(store.findings(&snapshot)?);
        }

        let (mut rejections, mut warnings, mut existing) = (Vec::new(), Vec::new(), Vec::new());
        for finding in &findings {
            let mut report = FindingReport::from(finding);
            if !counts_against(finding) {
                existing.push(report);
                continue;
            }
            report.path = store.shortest_path(&snapshot, &finding.purl)?;
            let rejects = finding.advisory.kind == AdvisoryKind::Vulnerability
                && finding.active != Some(false);
            if rejects {
                rejections.push(report);
            } else {
                warnings.push(report);
            }
        }
        let candidate_keys: BTreeSet<_> = findings.iter().map(finding_key).collect();
        let improvements = baseline_findings
            .iter()
            .filter(|f| {
                f.advisory.kind == AdvisoryKind::Vulnerability
                    && !candidate_keys.contains(&finding_key(f))
            })
            .map(FindingReport::from)
            .collect();

        let subject_version = candidate
            .state
            .packages
            .iter()
            .filter(|p| p.id.name == change.name)
            .filter_map(|p| semver::Version::parse(&p.id.version).ok())
            .max();
        Ok(Evaluated {
            change: change.clone(),
            state_digest: candidate.state.digest(),
            diff: store.diff(baseline, &snapshot)?,
            snapshot,
            precise,
            rejections,
            warnings,
            existing,
            improvements,
            subject_version,
        })
    }

    /// `depvigil check`: simulates one to four candidate changes, judges
    /// each against the policy, records a decision for each, and marks the
    /// best acceptable one.
    pub fn check(&self, changes: &[ProposedChange]) -> Result<CheckReport> {
        if changes.is_empty() || changes.len() > MAX_CANDIDATES {
            return Err(Error::InvalidInput(format!(
                "check takes one to {MAX_CANDIDATES} candidates, got {}",
                changes.len()
            )));
        }
        let policy = Policy::load(&self.project)?;
        // Reading the current lockfile runs no package-manager command.
        let current = self
            .ecosystem
            .current_state(&self.project, Precision::Lockfile)?;
        let mut store = Store::open(&self.project)?;
        let project = self.project_name(&mut store)?;
        let baseline = store
            .baseline()?
            .ok_or_else(|| Error::NotInitialised(self.project.clone()))?;
        if baseline.digest != current.digest() {
            return Err(Error::StaleBaseline);
        }
        let load = store.latest_load()?.map(|l| l.id);
        let now = self.clock.now();
        let baseline_findings = live(store.findings(&baseline.id)?);

        let context = CheckContext {
            project: &project,
            policy,
            baseline: &baseline.id,
            baseline_findings: &baseline_findings,
            now: &now,
            load: load.as_deref(),
        };
        let mut evaluated = Vec::new();
        for change in changes {
            evaluated.push(self.evaluate(&mut store, change, &context)?);
        }

        // Rank the acceptable candidates: no warnings first, then fewest new
        // packages, then the highest version.
        let rank = |e: &Evaluated| {
            (
                !e.warnings.is_empty(),
                e.diff.added.len(),
                std::cmp::Reverse(e.subject_version.clone()),
            )
        };
        let mut acceptable: Vec<usize> = (0..evaluated.len())
            .filter(|&i| evaluated[i].rejections.is_empty())
            .collect();
        acceptable.sort_by_key(|&i| rank(&evaluated[i]));
        let best = acceptable.first().copied();
        let best_reason = best.map(|best| match acceptable.get(1) {
            None if evaluated.len() == 1 => "it is the only candidate".to_string(),
            None => "it is the only acceptable candidate".to_string(),
            Some(&second) => {
                let (a, b) = (&evaluated[best], &evaluated[second]);
                if a.warnings.is_empty() != b.warnings.is_empty() {
                    "it has no warnings".to_string()
                } else if a.diff.added.len() != b.diff.added.len() {
                    format!(
                        "it adds the fewest new packages ({} against {})",
                        a.diff.added.len(),
                        b.diff.added.len()
                    )
                } else if a.subject_version != b.subject_version {
                    match &a.subject_version {
                        Some(version) => format!("it resolves to the highest version ({version})"),
                        None => "it was listed first".to_string(),
                    }
                } else {
                    "it was listed first among equals".to_string()
                }
            }
        });

        // Record the best last, so "the latest approved decision" is the one
        // the caller is told to apply.
        let mut order: Vec<usize> = (0..evaluated.len()).filter(|&i| Some(i) != best).collect();
        order.extend(best);
        let mut reports: Vec<Option<CandidateReport>> = evaluated.iter().map(|_| None).collect();
        for index in order {
            let e = &evaluated[index];
            let verdict = if e.rejections.is_empty() {
                "approved"
            } else {
                "rejected"
            };
            let precision = if e.precise {
                Precision::Full
            } else {
                Precision::Lockfile
            };
            let details = serde_json::json!({
                "precise_step": e.precise,
                "rejections": e.rejections,
                "warnings": e.warnings,
                "existing": e.existing,
                "improvements": e.improvements,
                "diff": e.diff,
            });
            let evidence: BTreeSet<String> = e
                .rejections
                .iter()
                .chain(&e.warnings)
                .map(|f| f.advisory.clone())
                .collect();
            let decision = store.record_decision(
                &policy.id(),
                policy.rule(),
                &NewDecision {
                    verdict: verdict.to_string(),
                    decided_at: now.clone(),
                    digest: e.state_digest.clone(),
                    change: e.change.to_string(),
                    subject: e.change.name.clone(),
                    mode: policy.mode.as_str().to_string(),
                    precision: precision.as_str().to_string(),
                    details: details.to_string(),
                    baseline: baseline.id.clone(),
                    candidate: e.snapshot.clone(),
                    evidence: evidence.iter().cloned().collect(),
                    load: load.clone(),
                },
            )?;
            mirror::append(
                &self.project,
                &serde_json::json!({
                    "type": "decision",
                    "id": decision,
                    "verdict": verdict,
                    "decided_at": now,
                    "change": e.change.to_string(),
                    "digest": e.state_digest,
                    "mode": policy.mode.as_str(),
                    "precision": precision.as_str(),
                    "baseline": baseline.id,
                    "candidate": e.snapshot,
                    "advisory_load": load,
                    "rejected_by": e.rejections.iter().map(|f| f.advisory.as_str()).collect::<BTreeSet<_>>(),
                    "warnings": e.warnings.iter().map(|f| f.advisory.as_str()).collect::<BTreeSet<_>>(),
                }),
            )?;
            reports[index] = Some(CandidateReport {
                spec: e.change.spec(),
                change: e.change.to_string(),
                verdict: verdict.to_string(),
                decision,
                snapshot: e.snapshot.clone(),
                digest: e.state_digest.clone(),
                precise_step: e.precise,
                rejections: e.rejections.clone(),
                warnings: e.warnings.clone(),
                existing: e.existing.clone(),
                improvements: e.improvements.clone(),
                diff: e.diff.clone(),
                best: Some(index) == best,
            });
        }
        store.checkpoint()?;

        Ok(CheckReport {
            baseline: baseline.id,
            mode: policy.mode,
            advisory_load: load,
            best: best.map(|i| evaluated[i].change.spec()),
            best_reason,
            candidates: reports.into_iter().flatten().collect(),
            commands: self.ecosystem.tool_invocations(),
        })
    }

    /// `depvigil verify`: recomputes the digest of the workspace and
    /// compares it with the latest approved decision. No package-manager
    /// command runs.
    pub fn verify(&self) -> Result<VerifyReport> {
        let current = self
            .ecosystem
            .current_state(&self.project, Precision::Lockfile)?;
        let mut store = Store::open_read_only(&self.project)?;
        let decisions = store.decisions()?;
        let decision = decisions
            .iter()
            .rev()
            .find(|d| d.verdict == "approved")
            .ok_or_else(|| {
                Error::InvalidInput("there is no approved decision to verify against".to_string())
            })?;
        let current_digest = current.digest();
        let matches = current_digest == decision.digest;

        let mut differences = Vec::new();
        if !matches {
            let approved = store.snapshot(&decision.candidate)?.ok_or_else(|| {
                Error::Corrupt(format!("decision {} has no snapshot", decision.id))
            })?;
            let current_text = current.canonical();
            let approved_lines: BTreeSet<&str> = approved.canonical.lines().collect();
            let current_lines: BTreeSet<&str> = current_text.lines().collect();
            let mut push = |line: &str, only_in: &str| {
                let (kind, detail) = line.split_once(' ').unwrap_or((line, ""));
                differences.push(Difference {
                    only_in: only_in.to_string(),
                    kind: kind.to_string(),
                    detail: detail.to_string(),
                });
            };
            for line in current_lines.difference(&approved_lines) {
                push(line, "workspace");
            }
            for line in approved_lines.difference(&current_lines) {
                push(line, "approved");
            }
            differences.sort_by(|a, b| {
                (&a.kind, &a.detail, &a.only_in).cmp(&(&b.kind, &b.detail, &b.only_in))
            });
        }
        Ok(VerifyReport {
            decision: decision.id.clone(),
            change: decision.change.clone(),
            status: DecisionStatus::of(decision),
            approved_digest: decision.digest.clone(),
            current_digest,
            matches,
            differences,
        })
    }

    /// `depvigil recheck`: Living Approvals. Finds approved decisions that
    /// an advisory has come to affect since they were made, records one
    /// invalidation per decision and advisory, and reports decisions whose
    /// only invalidating advisories have been withdrawn.
    pub fn recheck(&self) -> Result<RecheckReport> {
        let mut store = Store::open(&self.project)?;
        let now = self.clock.now();
        let decisions = store.decisions()?;
        let change_of = |id: &str| {
            decisions
                .iter()
                .find(|d| d.id == id)
                .map(|d| d.change.clone())
                .unwrap_or_default()
        };
        let mut report = RecheckReport {
            approved_decisions: decisions.iter().filter(|d| d.verdict == "approved").count(),
            ..RecheckReport::default()
        };

        // One group per decision and advisory pair (ADR 0004).
        let mut groups: BTreeMap<(String, String), Vec<_>> = BTreeMap::new();
        for pending in store.pending_invalidations()? {
            groups
                .entry((pending.decision.clone(), pending.advisory.clone()))
                .or_default()
                .push(pending);
        }
        for ((decision, advisory), hits) in groups {
            let first = &hits[0];
            // A version known to be outside the resolve graph cannot invalidate.
            let blocking = hits.iter().find(|hit| hit.active != Some(false));
            let shown = blocking.unwrap_or(first);
            let mut entry = RecheckEntry {
                decision: decision.clone(),
                change: change_of(&decision),
                advisory: advisory.clone(),
                kind: first.kind,
                summary: first.summary.clone(),
                published: first.published.clone(),
                first_seen_at: Some(shown.first_seen_at.clone()),
                purl: shown.purl.clone(),
                path: store.shortest_path(&shown.snapshot, &shown.purl)?,
                new: false,
                note: None,
            };
            if first.kind == AdvisoryKind::Notice {
                entry.note = Some("a notice warns and does not invalidate".to_string());
                report.warnings.push(entry);
            } else if blocking.is_none() {
                entry.note =
                    Some("the affected package is locked but not in the resolve graph".to_string());
                report.warnings.push(entry);
            } else {
                if !first.recorded {
                    let reason = "An advisory came to affect a version in the approved snapshot after the approval";
                    let (id, created) = store.record_invalidation(
                        &decision,
                        &advisory,
                        &entry.purl,
                        &now,
                        reason,
                    )?;
                    if created {
                        mirror::append(
                            &self.project,
                            &serde_json::json!({
                                "type": "invalidation",
                                "id": id,
                                "decision": decision,
                                "advisory": advisory,
                                "purl": entry.purl,
                                "at": now,
                                "reason": reason,
                            }),
                        )?;
                    }
                    entry.new = created;
                }
                report.invalidated.push(entry);
            }
        }

        // Approvals whose every invalidating advisory has been withdrawn.
        for decision in store.decisions()? {
            if DecisionStatus::of(&decision) != DecisionStatus::Restored {
                continue;
            }
            for invalidation in &decision.invalidations {
                report.restored.push(RecheckEntry {
                    decision: decision.id.clone(),
                    change: decision.change.clone(),
                    advisory: invalidation.advisory.clone(),
                    kind: AdvisoryKind::Vulnerability,
                    summary: invalidation.advisory_summary.clone(),
                    published: invalidation.advisory_published.clone(),
                    first_seen_at: None,
                    purl: invalidation.purl.clone(),
                    path: store.shortest_path(&decision.candidate, &invalidation.purl)?,
                    new: false,
                    note: invalidation
                        .advisory_withdrawn
                        .as_ref()
                        .map(|at| format!("the advisory was withdrawn on {at}")),
                });
            }
        }
        store.checkpoint()?;
        Ok(report)
    }

    /// `depvigil log`: every decision with its verdict, date and current
    /// status, oldest first.
    pub fn log(&self) -> Result<Vec<LogEntry>> {
        let mut store = Store::open_read_only(&self.project)?;
        Ok(store.decisions()?.iter().map(LogEntry::from).collect())
    }

    /// `depvigil why`: the dependency paths to a package in the baseline,
    /// and the past decisions that mention it.
    pub fn why(&self, package: &str) -> Result<WhyReport> {
        let mut store = Store::open_read_only(&self.project)?;
        let baseline = store
            .baseline()?
            .ok_or_else(|| Error::NotInitialised(self.project.clone()))?;
        let mut versions = Vec::new();
        for (purl, active) in store.versions_of(&baseline.id, package)? {
            let path = store.shortest_path(&baseline.id, &purl)?;
            versions.push(WhyVersion { purl, active, path });
        }
        let mentioned = store.decisions_mentioning(package)?;
        let decisions = store
            .decisions()?
            .iter()
            .filter(|d| mentioned.contains(&d.id))
            .map(LogEntry::from)
            .collect();
        Ok(WhyReport {
            package: package.to_string(),
            baseline: baseline.id,
            versions,
            decisions,
        })
    }
}
