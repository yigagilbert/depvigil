//! Test 3 (gate): isolated candidate resolution.
//!
//! Method under test: copy only the manifests of a workspace into a temporary
//! directory, apply a dependency change there with `cargo add` or
//! `cargo update --precise`, and read the `resolve` graph from
//! `cargo metadata`. Ground truth is the same change applied to a full copy
//! of the fixture. Nothing is ever built, and the fixture is never touched.
//!
//! Each case runs four resolutions against one fresh `CARGO_HOME`:
//!   A  isolated, cold cache, online   (measures what Cargo downloads)
//!   T  full copy, online              (ground truth)
//!   B  isolated, warm cache, online   (repeatability)
//!   C  isolated, warm cache, offline
//!
//! Run: `cargo test --test t3_isolated -- --nocapture`

mod common;

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use common::{
    add_stubs, cached_crates, copy_full, copy_manifests, diff, downloaded, file_sha256,
    find_named, fixture, git_status, parse_metadata, resolve_of, run_cargo, Resolve, BUILD_MARKER,
};

struct Case {
    id: &'static str,
    label: &'static str,
    fixture: &'static str,
    /// The dependency change, as arguments to an allowed Cargo subcommand.
    change: &'static [&'static str],
    /// Package-id fragments that must be present after the change.
    expect_present: &'static [&'static str],
    /// Package-id fragments that must be absent after the change.
    expect_absent: &'static [&'static str],
    /// A fragment that must appear in some resolved edge.
    expect_edge: (&'static str, &'static str),
    /// Counted among the five cases of the brief, or an extra.
    counted: bool,
}

const CASES: &[Case] = &[
    Case {
        id: "1",
        label: "add a crate that is not in the tree",
        fixture: "small",
        change: &["add", "getrandom@=0.2.15", "-p", "small-app"],
        expect_present: &["#getrandom@0.2.15", "#wasi@"],
        expect_absent: &[],
        expect_edge: ("app#small-app@0.1.0 -> ", "#getrandom@0.2.15"),
        counted: true,
    },
    Case {
        id: "2",
        label: "upgrade an existing direct dependency",
        fixture: "small",
        change: &["add", "smallvec@=1.6.1", "-p", "small-app"],
        expect_present: &["#smallvec@1.6.1"],
        expect_absent: &["#smallvec@1.6.0"],
        expect_edge: ("app#small-app@0.1.0 -> ", "#smallvec@1.6.1"),
        counted: true,
    },
    Case {
        id: "3",
        label: "add a crate with a non-default feature",
        fixture: "small",
        change: &["add", "serde@=1.0.210", "-p", "small-app", "--features", "derive"],
        expect_present: &["#serde@1.0.210", "#serde_derive@1.0.210", "#syn@"],
        expect_absent: &[],
        expect_edge: ("#serde@1.0.210 -> ", "#serde_derive@1.0.210"),
        counted: true,
    },
    Case {
        id: "4",
        label: "add a crate to one member of the workspace",
        fixture: "small",
        change: &["add", "either@=1.13.0", "-p", "small-core"],
        expect_present: &["#either@1.13.0"],
        expect_absent: &[],
        expect_edge: ("core#small-core@0.1.0 -> ", "#either@1.13.0"),
        counted: true,
    },
    Case {
        id: "5",
        label: "add a crate that is not in the tree",
        fixture: "public",
        change: &["add", "petgraph@=0.6.5"],
        expect_present: &["#petgraph@0.6.5", "#fixedbitset@"],
        expect_absent: &[],
        expect_edge: ("#bat@0.25.0 -> ", "#petgraph@0.6.5"),
        counted: true,
    },
    Case {
        id: "extra",
        label: "change a locked version with cargo update --precise",
        fixture: "small",
        change: &["update", "itoa", "--precise", "1.0.10"],
        expect_present: &["#itoa@1.0.10"],
        expect_absent: &[],
        expect_edge: ("app#small-app@0.1.0 -> ", "#itoa@1.0.10"),
        counted: false,
    },
];

struct Run {
    resolve: Resolve,
    lock_sha: String,
    wall: Duration,
    stderr: String,
    /// Cargo's complaint about the bare manifest copy, if stubs were needed.
    stub_reason: Option<String>,
    stubs: Vec<String>,
    /// Violations of "nothing compiled" and "no build script ran".
    violations: Vec<String>,
}

fn with_offline<'a>(args: &[&'a str], offline: bool) -> Vec<&'a str> {
    let mut args = args.to_vec();
    if offline {
        args.push("--offline");
    }
    args
}

/// Applies `change` in `dir` and reads the resolve graph. Never builds.
fn change_and_resolve(
    dir: &Path,
    cargo_home: &Path,
    change: &[&str],
    offline: bool,
) -> Result<(Resolve, String, String), String> {
    let applied = run_cargo(dir, cargo_home, &with_offline(change, offline));
    if !applied.success {
        return Err(format!("cargo {} failed:\n{}", change.join(" "), applied.stderr));
    }
    let metadata = run_cargo(
        dir,
        cargo_home,
        &with_offline(&["metadata", "--format-version", "1"], offline),
    );
    if !metadata.success {
        return Err(format!("cargo metadata failed:\n{}", metadata.stderr));
    }
    let resolve = resolve_of(&parse_metadata(&metadata.stdout, dir));
    let lock_sha = file_sha256(&dir.join("Cargo.lock"));
    Ok((resolve, lock_sha, applied.stderr + &metadata.stderr))
}

fn violations_in(dir: &Path) -> Vec<String> {
    let mut violations = Vec::new();
    for target in find_named(dir, "target") {
        violations.push(format!("target directory exists: {}", target.display()));
    }
    for marker in find_named(dir, BUILD_MARKER) {
        violations.push(format!("build script ran: {}", marker.display()));
    }
    violations
}

/// The method under test.
fn isolated(fixture_root: &Path, cargo_home: &Path, change: &[&str], offline: bool) -> Result<Run, String> {
    let start = Instant::now();
    let temp = tempfile::tempdir().expect("tempdir");
    let dir = temp.path();
    copy_manifests(fixture_root, dir);

    // Does Cargo accept the bare manifests? This probe needs no network.
    let probe = run_cargo(
        dir,
        cargo_home,
        &["metadata", "--format-version", "1", "--no-deps", "--offline"],
    );
    let (stub_reason, stubs) = if probe.success {
        (None, Vec::new())
    } else {
        (Some(probe.stderr.trim().to_string()), add_stubs(dir))
    };

    let (resolve, lock_sha, stderr) = change_and_resolve(dir, cargo_home, change, offline)?;
    Ok(Run {
        resolve,
        lock_sha,
        wall: start.elapsed(),
        stderr,
        stub_reason,
        stubs,
        violations: violations_in(dir),
    })
}

/// Ground truth: the same change on a full copy of the fixture.
fn ground_truth(fixture_root: &Path, cargo_home: &Path, change: &[&str]) -> Result<Run, String> {
    let start = Instant::now();
    let temp = tempfile::tempdir().expect("tempdir");
    let dir = temp.path();
    copy_full(fixture_root, dir);
    let (resolve, lock_sha, stderr) = change_and_resolve(dir, cargo_home, change, false)?;
    Ok(Run {
        resolve,
        lock_sha,
        wall: start.elapsed(),
        stderr,
        stub_reason: None,
        stubs: Vec::new(),
        violations: violations_in(dir),
    })
}

fn dir_size(path: &Path) -> u64 {
    if !path.exists() {
        return 0;
    }
    let meta = fs::symlink_metadata(path).expect("metadata");
    if !meta.is_dir() {
        return meta.len();
    }
    fs::read_dir(path)
        .expect("read_dir")
        .map(|entry| dir_size(&entry.expect("entry").path()))
        .sum()
}

/// Runs one case and returns the failed checks. Empty means the case passed.
fn run_case(case: &Case) -> Vec<String> {
    let tag = format!("[t3 case {}]", case.id);
    let mut failures = Vec::new();
    let mut check = |ok: bool, what: String| {
        println!("{tag} {} {what}", if ok { "PASS" } else { "FAIL" });
        if !ok {
            failures.push(what);
        }
    };

    let fixture_root = fixture(case.fixture);
    let fixture_lock = fixture_root.join("Cargo.lock");
    let lock_before = file_sha256(&fixture_lock);
    println!("{tag} {}: cargo {} (fixture `{}`)", case.label, case.change.join(" "), case.fixture);
    assert_eq!(git_status(&fixture_root), "", "fixture must be clean before the case");

    let home = tempfile::tempdir().expect("cargo home");
    let home = home.path();

    let run = |name: &str, result: Result<Run, String>| -> Option<Run> {
        match result {
            Ok(run) => Some(run),
            Err(error) => {
                println!("{tag} run {name} failed: {error}");
                None
            }
        }
    };
    let cold = run("A (isolated, cold)", isolated(&fixture_root, home, case.change, false));
    let crates_after_cold = cached_crates(home);
    let cache_bytes = dir_size(&home.join("registry").join("cache"));
    let truth = run("T (ground truth)", ground_truth(&fixture_root, home, case.change));
    let crates_after_truth = cached_crates(home);
    let warm = run("B (isolated, warm)", isolated(&fixture_root, home, case.change, false));
    let offline = run("C (isolated, offline)", isolated(&fixture_root, home, case.change, true));

    let (Some(cold), Some(truth), Some(warm), Some(offline)) = (cold, truth, warm, offline) else {
        check(false, "all four resolutions completed".to_string());
        return failures;
    };

    // Numbers.
    println!(
        "{tag} resolved packages: {}, edges: {}",
        truth.resolve.packages.len(),
        truth.resolve.edges.len()
    );
    println!(
        "{tag} wall time: isolated cold {:?}, isolated warm {:?}, isolated offline {:?}, ground truth {:?}",
        cold.wall, warm.wall, offline.wall, truth.wall
    );
    match &cold.stub_reason {
        Some(reason) => {
            println!("{tag} stubs needed: yes, {} created: {}", cold.stubs.len(), cold.stubs.join(", "));
            println!("{tag} Cargo's reason: {}", reason.replace('\n', " | "));
        }
        None => println!("{tag} stubs needed: no"),
    }
    let used_network = cold.stderr.contains("Updating") || cold.stderr.contains("Downloaded");
    let fetched = downloaded(&cold.stderr);
    println!(
        "{tag} network on first run: {used_network}; index updated: {}; crates downloaded: {} ({} bytes of .crate archives)",
        cold.stderr.contains("Updating crates.io index"),
        crates_after_cold.len(),
        cache_bytes
    );
    if crates_after_cold.len() <= 40 {
        let list: Vec<&str> = crates_after_cold.iter().map(String::as_str).collect();
        println!("{tag} downloaded: {}", list.join(", "));
    }
    println!(
        "{tag} `Downloaded` lines printed by Cargo: {}; extra crates fetched by ground truth: {}",
        fetched.len(),
        crates_after_truth.difference(&crates_after_cold).count()
    );
    println!(
        "{tag} warm run network activity: index updated {}, downloads {}",
        warm.stderr.contains("Updating crates.io index"),
        downloaded(&warm.stderr).len()
    );

    // The change took effect at all.
    let has = |resolve: &Resolve, fragment: &str| resolve.packages.iter().any(|p| p.contains(fragment));
    let effective = case.expect_present.iter().all(|f| has(&cold.resolve, f))
        && !case.expect_absent.iter().any(|f| has(&cold.resolve, f))
        && cold
            .resolve
            .edges
            .iter()
            .any(|e| e.contains(case.expect_edge.0) && e.contains(case.expect_edge.1));
    check(effective, "change is visible in the isolated resolve graph".to_string());

    // Correctness: same package set and same edges as the ground truth.
    let differences = diff("isolated", &cold.resolve, "ground truth", &truth.resolve);
    for line in &differences {
        println!("{tag}   {line}");
    }
    check(differences.is_empty(), "correctness: isolated resolve equals ground truth".to_string());
    println!(
        "{tag} Cargo.lock identical to ground truth: {}",
        cold.lock_sha == truth.lock_sha
    );

    // Repeatable.
    check(
        cold.resolve == warm.resolve && cold.lock_sha == warm.lock_sha,
        "repeatable: second isolated run gives the same result".to_string(),
    );

    // Offline, with the cache the first run warmed.
    let offline_differences = diff("offline", &offline.resolve, "ground truth", &truth.resolve);
    for line in &offline_differences {
        println!("{tag}   {line}");
    }
    check(
        offline_differences.is_empty(),
        "offline: isolated --offline resolve equals ground truth".to_string(),
    );

    // Nothing compiled, no build script ran.
    let mut violations: Vec<String> = Vec::new();
    for run in [&cold, &truth, &warm, &offline] {
        violations.extend(run.violations.iter().cloned());
    }
    violations.extend(violations_in(&fixture_root));
    for violation in &violations {
        println!("{tag}   {violation}");
    }
    check(violations.is_empty(), "nothing compiled and no build script ran".to_string());

    // Original untouched.
    let status = git_status(&fixture_root);
    check(
        status.is_empty() && file_sha256(&fixture_lock) == lock_before,
        format!("original untouched: git status clean, Cargo.lock sha256 {lock_before}"),
    );
    failures
}

#[test]
fn t3_isolated_candidate_resolution() {
    let mut counted_passed = 0;
    let mut counted_total = 0;
    let mut summary = Vec::new();
    for case in CASES {
        let failures = run_case(case);
        if case.counted {
            counted_total += 1;
            if failures.is_empty() {
                counted_passed += 1;
            }
        }
        summary.push((case.id, failures));
    }
    println!("[t3] correctness and safety: {counted_passed} of {counted_total} counted cases pass");
    for (id, failures) in &summary {
        if failures.is_empty() {
            println!("[t3] case {id}: PASS");
        } else {
            println!("[t3] case {id}: FAIL ({})", failures.join("; "));
        }
    }
    assert!(
        summary.iter().all(|(_, failures)| failures.is_empty()),
        "Test 3 has failing cases; see the output above"
    );
}
