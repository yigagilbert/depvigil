//! The `depvigil` command line.
//!
//! Exit codes: 0 success; 1 a negative result (a rejected candidate, a
//! digest mismatch, an invalidated approval); 2 a usage error; 3 an
//! unsupported workspace; 4 any other error.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use serde::Serialize;

use depvigil::ecosystem::cargo::CargoEcosystem;
use depvigil::engine::{
    CandidateReport, CheckReport, DecisionStatus, FindingReport, InitReport, LoadReport, LogEntry,
    RecheckEntry, RecheckReport, VerifyReport, WhyReport,
};
use depvigil::model::ProposedChange;
use depvigil::time::Clock;
use depvigil::{Engine, Error};

/// Set to a UTC timestamp to fix the clock. A testing and demonstration aid.
const CLOCK_VARIABLE: &str = "DEPVIGIL_NOW";

#[derive(Parser)]
#[command(
    name = "depvigil",
    version,
    about = "Dependency decisions that stay true: simulate a change, record an approval, and invalidate it when the evidence changes"
)]
struct Cli {
    /// The project directory (the workspace root).
    #[arg(long, global = true, default_value = ".")]
    project: PathBuf,

    /// Print machine-readable JSON.
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create the store and record the current workspace as the baseline.
    Init,
    /// Work with security advisories.
    Advisories {
        #[command(subcommand)]
        command: AdvisoryCommand,
    },
    /// Simulate adding one to four candidate dependencies and record a decision for each.
    Check {
        /// Candidates as `name` or `name@version`.
        #[arg(required = true, num_args = 1..=4)]
        candidates: Vec<String>,
        /// Features to enable, separated by commas.
        #[arg(long, value_delimiter = ',')]
        features: Vec<String>,
        /// Turn off the candidate's default features.
        #[arg(long)]
        no_default_features: bool,
        /// Add the candidate as an optional dependency.
        #[arg(long)]
        optional: bool,
        /// The workspace member to add the dependency to.
        #[arg(long, short = 'p')]
        package: Option<String>,
        /// Do not use the network; resolve from the local Cargo cache only.
        #[arg(long)]
        offline: bool,
    },
    /// Compare the workspace with the latest approved decision.
    Verify,
    /// Re-examine past approvals against the advisories known now.
    Recheck,
    /// List every decision with its verdict, date and current status.
    Log,
    /// Show why a package is in the baseline, and the decisions that mention it.
    Why {
        /// The package name.
        package: String,
    },
}

#[derive(Subcommand)]
enum AdvisoryCommand {
    /// Load OSV advisories from a directory, a .jsonl file or a .jsonl.gz file.
    Load {
        /// Where the advisories are.
        path: PathBuf,
        /// Load only advisories published before this date. This simulates
        /// an earlier point in time and is meant for demonstrations.
        #[arg(long, value_name = "DATE")]
        published_before: Option<String>,
    },
}

fn short(purl: &str) -> &str {
    let rest = purl.strip_prefix("pkg:cargo/").unwrap_or(purl);
    rest.split("?source=path:").next().unwrap_or(rest)
}

fn path_text(path: &Option<Vec<String>>) -> String {
    match path {
        Some(path) => path
            .iter()
            .map(|p| short(p))
            .collect::<Vec<_>>()
            .join(" -> "),
        None => "(no path from a workspace member)".to_string(),
    }
}

fn print_finding(finding: &FindingReport, indent: &str) {
    let activity = match finding.active {
        Some(false) => " [inactive: locked but not in the resolve graph]",
        _ => "",
    };
    let kind = finding.informational.as_deref().unwrap_or("vulnerability");
    println!(
        "{indent}{} ({kind}, published {}) affects {}{activity}",
        finding.advisory,
        &finding.published[..10.min(finding.published.len())],
        short(&finding.purl)
    );
    if !finding.summary.is_empty() {
        println!("{indent}  {}", finding.summary);
    }
    if finding.path.is_some() {
        println!("{indent}  path: {}", path_text(&finding.path));
    }
}

fn print_init(report: &InitReport) {
    if report.baseline_changed {
        println!(
            "Recorded baseline {} for project `{}`.",
            report.snapshot, report.project
        );
    } else {
        println!(
            "Baseline {} is already up to date; nothing was added.",
            report.snapshot
        );
    }
    let s = &report.stats;
    println!(
        "  {} packages ({} in the resolve graph, {} locked but inactive), {} dependency edges, {} workspace member(s)",
        s.packages, s.active, s.inactive, s.resolve_edges, s.roots
    );
    println!("  digest {}", report.digest);
}

fn print_load(report: &LoadReport) {
    let load = &report.load;
    println!(
        "Loaded {} advisories as {} ({} vulnerabilities, {} notices, {} withdrawn).",
        load.advisory_count, load.id, report.vulnerabilities, report.notices, report.withdrawn
    );
    println!(
        "  source: {} {} at {} ({})",
        load.repository, load.branch, load.commit, load.source_date
    );
    println!(
        "  {} new advisory-to-version link(s), {} removed",
        report.new_affects, report.removed_affects
    );
    if let Some(cutoff) = &load.published_before {
        println!(
            "  NOTE: simulating the passage of time. Only advisories published before {} were loaded; {} later ones were left out.",
            &cutoff[..10],
            report.skipped_by_cutoff
        );
    }
}

fn print_candidate(candidate: &CandidateReport) {
    let marker = if candidate.best { "  <- best" } else { "" };
    println!(
        "{} {}: {} (decision {}){marker}",
        candidate.verdict.to_uppercase(),
        candidate.spec,
        candidate.change,
        candidate.decision
    );
    println!(
        "  step: {}",
        if candidate.precise_step {
            "lockfile check found a policy hit, so the full resolution ran"
        } else {
            "lockfile check only; cargo metadata was not needed"
        }
    );
    println!(
        "  changes: {} version(s) added, {} removed, {} package(s) changed",
        candidate.diff.added.len(),
        candidate.diff.removed.len(),
        candidate.diff.changed.len()
    );
    for finding in &candidate.rejections {
        println!("  rejected by:");
        print_finding(finding, "    ");
    }
    for finding in &candidate.warnings {
        println!("  warning:");
        print_finding(finding, "    ");
    }
    for finding in &candidate.improvements {
        println!(
            "  improvement: removes {} ({})",
            finding.advisory,
            short(&finding.purl)
        );
    }
    if !candidate.existing.is_empty() {
        println!(
            "  context: {} finding(s) already in the baseline, not counted against this candidate",
            candidate.existing.len()
        );
    }
    println!("  digest {}", candidate.digest);
}

fn print_check(report: &CheckReport) {
    for candidate in &report.candidates {
        print_candidate(candidate);
    }
    match (&report.best, &report.best_reason) {
        (Some(best), Some(reason)) if report.candidates.len() > 1 => {
            println!("Best acceptable candidate: {best}, because {reason}.");
        }
        (None, _) => println!("No candidate is acceptable."),
        _ => {}
    }
    match &report.advisory_load {
        Some(load) => println!("Judged against advisory load {load}, policy mode `{}`.", report.mode.as_str()),
        None => println!("No advisories are loaded, so nothing could be flagged. Run `depvigil advisories load`."),
    }
}

fn print_verify(report: &VerifyReport) {
    if report.matches {
        println!(
            "MATCH: the workspace is exactly what decision {} approved ({}).",
            report.decision, report.change
        );
    } else {
        println!(
            "MISMATCH: the workspace differs from decision {} ({}).",
            report.decision, report.change
        );
        for difference in &report.differences {
            let side = if difference.only_in == "workspace" {
                "only in the workspace"
            } else {
                "approved, but missing"
            };
            println!("  {} {side}: {}", difference.kind, difference.detail);
        }
    }
    println!("  approved digest {}", report.approved_digest);
    println!("  current digest  {}", report.current_digest);
    if report.status != DecisionStatus::Valid {
        println!(
            "  note: decision {} is currently {}",
            report.decision,
            report.status.as_str()
        );
    }
}

fn print_recheck_entry(entry: &RecheckEntry) {
    println!(
        "  {} ({}): {} affects {}",
        entry.decision,
        entry.change,
        entry.advisory,
        short(&entry.purl)
    );
    if !entry.summary.is_empty() {
        println!("    {}", entry.summary);
    }
    match &entry.first_seen_at {
        Some(seen) => println!(
            "    published {}, first seen by this store {}",
            &entry.published[..10],
            seen
        ),
        None => println!("    published {}", &entry.published[..10]),
    }
    println!("    path: {}", path_text(&entry.path));
    if let Some(note) = &entry.note {
        println!("    note: {note}");
    }
}

fn print_recheck(report: &RecheckReport) {
    println!(
        "Rechecked {} approved decision(s).",
        report.approved_decisions
    );
    if report.invalidated.is_empty() && report.warnings.is_empty() && report.restored.is_empty() {
        println!("Every approval still holds.");
    }
    if !report.invalidated.is_empty() {
        let new = report.invalidated.iter().filter(|e| e.new).count();
        println!(
            "INVALIDATED ({new} new, {} standing):",
            report.invalidated.len() - new
        );
        report.invalidated.iter().for_each(print_recheck_entry);
    }
    if !report.warnings.is_empty() {
        println!("WARNINGS (no approval is invalidated):");
        report.warnings.iter().for_each(print_recheck_entry);
    }
    if !report.restored.is_empty() {
        println!("RESTORED (the invalidating advisory was withdrawn):");
        report.restored.iter().for_each(print_recheck_entry);
    }
}

fn print_log(entries: &[LogEntry]) {
    if entries.is_empty() {
        println!("No decisions yet.");
    }
    for entry in entries {
        let invalidated = if entry.invalidated_by.is_empty() {
            String::new()
        } else {
            format!(" by {}", entry.invalidated_by.join(", "))
        };
        println!(
            "{}  {}  {:<8}  {}{}  {}",
            entry.id,
            entry.decided_at,
            entry.verdict,
            entry.status.as_str(),
            invalidated,
            entry.change
        );
    }
}

fn print_why(report: &WhyReport) {
    if report.versions.is_empty() {
        println!("`{}` is not in the baseline.", report.package);
    }
    for version in &report.versions {
        let activity = if version.active == Some(false) {
            " [inactive]"
        } else {
            ""
        };
        println!("{}{activity}", short(&version.purl));
        println!("  path: {}", path_text(&version.path));
    }
    if !report.decisions.is_empty() {
        println!("Decisions that mention `{}`:", report.package);
        print_log(&report.decisions);
    }
}

fn emit<T: Serialize>(json: bool, report: &T, text: impl FnOnce(&T)) -> Result<(), Error> {
    if json {
        let rendered = serde_json::to_string_pretty(report)
            .map_err(|e| Error::InvalidInput(format!("could not render the report: {e}")))?;
        println!("{rendered}");
    } else {
        text(report);
    }
    Ok(())
}

/// Runs the command and returns whether its result is a negative one.
fn run(cli: &Cli) -> Result<bool, Error> {
    let clock = match std::env::var(CLOCK_VARIABLE) {
        Ok(instant) => Clock::fixed(&instant)?,
        Err(_) => Clock::System,
    };
    let offline = matches!(&cli.command, Command::Check { offline: true, .. });
    let engine =
        Engine::new(CargoEcosystem::new().offline(offline), &cli.project).with_clock(clock);
    match &cli.command {
        Command::Init => {
            emit(cli.json, &engine.init()?, print_init)?;
            Ok(false)
        }
        Command::Advisories {
            command:
                AdvisoryCommand::Load {
                    path,
                    published_before,
                },
        } => {
            emit(
                cli.json,
                &engine.load_advisories(path, published_before.as_deref())?,
                print_load,
            )?;
            Ok(false)
        }
        Command::Check {
            candidates,
            features,
            no_default_features,
            optional,
            package,
            ..
        } => {
            let mut changes = Vec::new();
            for candidate in candidates {
                let mut change = ProposedChange::parse(candidate)?;
                change.features = features.clone();
                change.no_default_features = *no_default_features;
                change.optional = *optional;
                change.member = package.clone();
                changes.push(change);
            }
            let report = engine.check(&changes)?;
            emit(cli.json, &report, print_check)?;
            Ok(report.best.is_none())
        }
        Command::Verify => {
            let report = engine.verify()?;
            emit(cli.json, &report, print_verify)?;
            Ok(!report.matches)
        }
        Command::Recheck => {
            let report = engine.recheck()?;
            emit(cli.json, &report, print_recheck)?;
            Ok(!report.invalidated.is_empty())
        }
        Command::Log => {
            emit(cli.json, &engine.log()?, |entries| print_log(entries))?;
            Ok(false)
        }
        Command::Why { package } => {
            emit(cli.json, &engine.why(package)?, print_why)?;
            Ok(false)
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(&cli) {
        Ok(false) => ExitCode::SUCCESS,
        Ok(true) => ExitCode::from(1),
        Err(error) => {
            let code = if matches!(error, Error::Unsupported(_)) {
                3
            } else {
                4
            };
            if cli.json {
                let unsupported = match &error {
                    Error::Unsupported(inputs) => serde_json::json!(inputs),
                    _ => serde_json::Value::Null,
                };
                println!(
                    "{}",
                    serde_json::json!({"error": {"message": error.to_string(), "exit_code": code, "unsupported": unsupported}})
                );
            }
            eprintln!("depvigil: {error}");
            ExitCode::from(code)
        }
    }
}
