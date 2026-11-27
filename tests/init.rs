//! `depvigil init` and the store it creates.
//!
//! Covers acceptance item 1, and migrates the store checks of Phase 0
//! Test 1 (single writer, shared reads) to the directory store.

mod common;

use common::{build_evidence, engine_at, ground_truth, project_from, tree_hashes};
use depvigil::store::Store;
use depvigil::Error;

const T0: &str = "2026-11-27T09:00:00Z";
const T1: &str = "2026-11-27T10:00:00Z";

#[test]
fn init_records_small_exactly_as_cargo_metadata_resolves_it() {
    let fixture = require_fixture!("small");
    let project = project_from(&fixture);
    let before = tree_hashes(&project.dir);
    let truth = ground_truth(&project_from(&fixture).dir);

    let report = engine_at(&project.dir, T0).init().expect("init");
    println!("small: {:?}", report.stats);
    assert!(report.baseline_changed);
    assert_eq!(
        report.stats.active as usize,
        truth.packages.len(),
        "package count equals cargo metadata"
    );
    assert_eq!(
        report.stats.resolve_edges as usize,
        truth.edges.len(),
        "edge count equals cargo metadata"
    );
    assert_eq!(report.stats.roots as usize, truth.members.len());
    assert_eq!(report.stats.packages, 15);
    assert_eq!(report.stats.inactive, 0);

    assert!(
        Store::path_in(&project.dir).is_dir(),
        "the store is a directory store"
    );
    assert_eq!(
        tree_hashes(&project.dir),
        before,
        "nothing outside .depvigil changed"
    );
    assert!(
        build_evidence(&project.dir).is_empty(),
        "nothing compiled, no build script ran"
    );
}

#[test]
fn init_twice_duplicates_nothing() {
    let fixture = require_fixture!("small");
    let project = project_from(&fixture);
    let first = engine_at(&project.dir, T0).init().expect("first init");
    let counts_after_first = Store::open(&project.dir)
        .expect("open")
        .counts()
        .expect("counts");

    let second = engine_at(&project.dir, T1).init().expect("second init");
    let counts_after_second = Store::open(&project.dir)
        .expect("open")
        .counts()
        .expect("counts");

    assert!(!second.baseline_changed);
    assert_eq!(second.snapshot, first.snapshot);
    assert_eq!(second.stats, first.stats);
    assert_eq!(
        counts_after_second, counts_after_first,
        "no node or relationship was added"
    );
    assert_eq!(counts_after_first["node:Snapshot"], 1);
    assert_eq!(counts_after_first["node:Version"], 15);
    assert_eq!(counts_after_first["relationship:BASELINE"], 1);
}

#[test]
fn init_records_bat_with_its_inactive_lockfile_only_packages() {
    let fixture = require_fixture!("bat");
    let project = project_from(&fixture);
    let truth = ground_truth(&project_from(&fixture).dir);

    let report = engine_at(&project.dir, T0).init().expect("init");
    println!("bat: {:?}", report.stats);
    assert_eq!(
        report.stats.active as usize,
        truth.packages.len(),
        "package count equals cargo metadata"
    );
    assert_eq!(
        report.stats.resolve_edges as usize,
        truth.edges.len(),
        "edge count equals cargo metadata"
    );
    assert_eq!(
        report.stats.packages, 208,
        "every lockfile package is stored"
    );
    assert_eq!(
        report.stats.inactive, 9,
        "lockfile-only packages are flagged inactive"
    );
    assert_eq!(report.stats.roots, 1);

    let again = engine_at(&project.dir, T1).init().expect("second init");
    assert!(!again.baseline_changed);
    assert_eq!(again.stats, report.stats);
}

#[test]
fn the_store_has_one_writer_and_allows_readers_beside_it() {
    let fixture = require_fixture!("small");
    let project = project_from(&fixture);
    engine_at(&project.dir, T0).init().expect("init");

    let mut writer = Store::open(&project.dir).expect("writer");
    match Store::open(&project.dir) {
        Err(Error::StoreLocked(_)) => {}
        other => panic!("a second writer must be refused as locked, got {other:?}"),
    }
    let mut reader = Store::open_read_only(&project.dir).expect("a reader opens beside the writer");
    assert_eq!(
        reader.counts().expect("reader counts"),
        writer.counts().expect("writer counts")
    );
}

#[test]
fn commands_before_init_say_so() {
    let fixture = require_fixture!("small");
    let project = project_from(&fixture);
    match engine_at(&project.dir, T0).log() {
        Err(Error::NotInitialised(_)) => {}
        other => panic!("expected a not-initialised error, got {other:?}"),
    }
}

#[test]
fn init_from_the_command_line_prints_json() {
    let fixture = require_fixture!("small");
    let project = project_from(&fixture);
    let output = common::run_cli(&project.dir, Some(T0), &["init", "--json"]);
    assert_eq!(output.code, 0, "{}", output.stderr);
    let report = output.json();
    assert_eq!(report["baseline_changed"], true);
    assert_eq!(report["stats"]["packages"], 15);
    assert_eq!(report["stats"]["roots"], 2);

    let text = common::run_cli(&project.dir, Some(T1), &["init"]);
    assert_eq!(text.code, 0);
    assert!(
        text.stdout.contains("already up to date"),
        "{}",
        text.stdout
    );
}

#[test]
fn the_store_does_not_depend_on_the_directory_name() {
    let fixture = require_fixture!("small");
    let project = project_from(&fixture);
    let first = engine_at(&project.dir, T0).init().expect("init");

    // The same project, store included, checked out under another name.
    let elsewhere = tempfile::tempdir().expect("tempdir");
    let moved = elsewhere.path().join("a-different-name");
    common::copy_tree(&project.dir, &moved);
    common::copy_tree(&project.dir.join(".depvigil"), &moved.join(".depvigil"));
    let second = engine_at(&moved, T1).init().expect("init elsewhere");
    assert!(!second.baseline_changed);
    assert_eq!(second.project, first.project);
    assert_eq!(second.snapshot, first.snapshot);
}

#[test]
fn another_process_can_read_but_not_write_while_the_store_is_held() {
    // Phase 0 Test 1 across a real process boundary: this process holds the
    // store for writing while the depvigil binary runs beside it.
    let fixture = require_fixture!("small");
    let project = project_from(&fixture);
    engine_at(&project.dir, T0).init().expect("init");
    let _writer = Store::open(&project.dir).expect("writer");

    let reader = common::run_cli(&project.dir, None, &["log"]);
    assert_eq!(
        reader.code, 0,
        "a read-only command works beside the writer: {}",
        reader.stderr
    );

    let second_writer = common::run_cli(&project.dir, None, &["recheck"]);
    assert_eq!(second_writer.code, 4);
    assert!(
        second_writer.stderr.contains("in use by another process"),
        "{}",
        second_writer.stderr
    );
}
