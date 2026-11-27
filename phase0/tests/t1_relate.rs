//! Test 1: Relate Rust crate basics.
//!
//! Checks persistence, parameters, transactions, read-only opens, the
//! single-writer rule and shared reads across processes. Every Cypher
//! statement is a constant; values only ever travel as parameters.
//!
//! Run: `cargo test --test t1_relate -- --nocapture`

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use relate::{error_info, Connection, Database, Error, Value};

const PROBE: &str = env!("CARGO_BIN_EXE_t1_probe");
const NODES: i64 = 1_000;

const CREATE_VERSION: &str =
    "CREATE (:Version {purl: $purl, source: $source, checksum: $checksum})";
const COUNT_VERSIONS: &str = "MATCH (v:Version) RETURN count(v) AS n";
const ENGINE_VERSION: &str = "CALL dbms.components() YIELD name, versions \
     WHERE name = 'Relate' RETURN versions[0] AS version";

fn version_params(i: i64) -> BTreeMap<String, Value> {
    BTreeMap::from([
        (
            "purl".to_string(),
            Value::String(format!("pkg:cargo/example-{i}@1.0.{i}").into()),
        ),
        (
            "source".to_string(),
            Value::String("registry+https://github.com/rust-lang/crates.io-index".into()),
        ),
        (
            "checksum".to_string(),
            Value::String(format!("{i:064x}").into()),
        ),
    ])
}

fn count(conn: &mut Connection) -> i64 {
    let result = conn.query(COUNT_VERSIONS).expect("count query");
    result.rows[0][0].as_int().expect("integer count")
}

fn write_versions(conn: &mut Connection, from: i64, to: i64) -> Duration {
    let start = Instant::now();
    conn.begin().expect("begin");
    for i in from..to {
        conn.query_with_params(CREATE_VERSION, version_params(i))
            .expect("create version");
    }
    conn.commit().expect("commit");
    start.elapsed()
}

/// Total size in bytes of a file, or of every file under a directory.
fn size_on_disk(path: &Path) -> u64 {
    let meta = fs::metadata(path).expect("metadata");
    if meta.is_file() {
        return meta.len();
    }
    fs::read_dir(path)
        .expect("read_dir")
        .map(|entry| size_on_disk(&entry.expect("dir entry").path()))
        .sum()
}

/// Runs the probe in a second process and returns its single output line.
fn probe(mode: &str, store: &Path) -> String {
    let output = Command::new(PROBE)
        .arg(mode)
        .arg(store)
        .output()
        .expect("spawn probe");
    assert!(output.status.success(), "probe exited with {}", output.status);
    String::from_utf8(output.stdout)
        .expect("utf-8 probe output")
        .trim()
        .to_string()
}

#[test]
fn t1_relate_basics() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file_store = dir.path().join("t1.rl");
    let dir_store = dir.path().join("t1_dir");
    // The crate takes a store location as `&str`.
    let file_uri = file_store.to_str().expect("utf-8 path");
    let dir_uri = dir_store.to_str().expect("utf-8 path");

    // Which engine is this?
    {
        let db = Database::open(":memory:").expect("open memory");
        let result = db.connect().expect("connect").query(ENGINE_VERSION).expect("version");
        println!(
            "[t1] engine version: {}",
            result.rows[0][0].as_string().unwrap_or("unknown")
        );
    }

    // Step 1: write 1,000 Version nodes with parameters in one transaction.
    {
        let db = Database::open(file_uri).expect("open t1.rl");
        let mut conn = db.connect().expect("connect");
        let elapsed = write_versions(&mut conn, 0, NODES);
        println!("[t1] write {NODES} nodes (.rl, one transaction): {elapsed:?}");
    }
    println!("[t1] .rl file size after close: {} bytes", size_on_disk(&file_store));

    // Step 2: reopen and count.
    let start = Instant::now();
    let db = Database::open(file_uri).expect("reopen t1.rl");
    let mut conn = db.connect().expect("connect");
    println!("[t1] reopen .rl: {:?}", start.elapsed());
    assert_eq!(count(&mut conn), NODES, "persistence: count after reopen");
    println!("[t1] PASS persistence: count after reopen = {NODES}");

    // Step 3: a rolled-back write leaves the count unchanged.
    conn.begin().expect("begin");
    conn.query_with_params(CREATE_VERSION, version_params(NODES))
        .expect("create inside transaction");
    assert_eq!(count(&mut conn), NODES + 1, "write visible inside its transaction");
    conn.rollback().expect("rollback");
    assert_eq!(count(&mut conn), NODES, "rollback: count unchanged");
    println!("[t1] PASS rollback: count unchanged at {NODES}");
    drop(conn);
    drop(db);

    // Step 4: a write on a read-only open fails with GQLSTATUS 42N18.
    {
        let db = Database::open_read_only(file_uri).expect("open read-only");
        let mut conn = db.connect().expect("connect");
        let err = conn
            .query_with_params(CREATE_VERSION, version_params(NODES))
            .expect_err("write on read-only must fail");
        let info = error_info(&err);
        println!("[t1] read-only write error: {err} (gql_status {})", info.gql_status);
        assert_eq!(info.gql_status, "42N18", "read-only write GQLSTATUS");
        assert_eq!(count(&mut conn), NODES);
        println!("[t1] PASS read-only: write refused with 42N18");
    }

    // Step 5: a second writable open of the .rl file from another process.
    {
        let db = Database::open(file_uri).expect("writer open");
        let mut conn = db.connect().expect("connect");

        let second_writer = probe("write", &file_store);
        println!("[t1] second writable open of .rl (other process): {second_writer}");
        assert!(
            second_writer.starts_with("ERR Locked "),
            "second writer must be refused as locked, got: {second_writer}"
        );

        // Same process: a second writable handle is refused the same way.
        match Database::open(file_uri) {
            Err(e @ Error::Locked(_)) => println!("[t1] second writable open (same process): {e}"),
            Err(e) => panic!("expected Locked, got: {e}"),
            Ok(_) => panic!("expected Locked, got a second writable handle"),
        }

        // For the design decision: can a .rl file be read beside its writer?
        let rl_reader = probe("read-only", &file_store);
        println!("[t1] read-only open of .rl beside its writer (other process): {rl_reader}");
        assert!(
            rl_reader.starts_with("ERR Locked "),
            "docs say a .rl file is exclusive to its writer, got: {rl_reader}"
        );

        // Nothing corrupted: the writer still works, and so does a reopen.
        assert_eq!(count(&mut conn), NODES);
        conn.query_with_params(CREATE_VERSION, version_params(NODES))
            .expect("writer still writes after refused opens");
        assert_eq!(count(&mut conn), NODES + 1);
    }
    {
        let db = Database::open(file_uri).expect("reopen after lock test");
        assert_eq!(count(&mut db.connect().expect("connect")), NODES + 1);
        println!("[t1] PASS single writer: second open locked, store intact after reopen");
    }

    // Step 6: a directory store, read from a second process beside its writer.
    // Any path that is not `:memory:` and does not end in `.rl` opens as a
    // directory store; there is no separate constructor.
    let db = Database::open(dir_uri).expect("open directory store");
    assert!(dir_store.is_dir(), "a path without .rl must create a directory store");
    let mut conn = db.connect().expect("connect");
    let elapsed = write_versions(&mut conn, 0, NODES);
    println!("[t1] write {NODES} nodes (directory store, one transaction): {elapsed:?}");

    let reader = probe("read-only", &dir_store);
    println!("[t1] read-only open of directory store beside its writer: {reader}");
    assert_eq!(reader, format!("OK {NODES}"), "shared read beside the writer");

    let dir_second_writer = probe("write", &dir_store);
    println!("[t1] second writable open of directory store: {dir_second_writer}");
    assert!(dir_second_writer.starts_with("ERR Locked "));

    // A follower opened before a later commit: does it see that commit?
    let mut follower = Command::new(PROBE)
        .arg("follow")
        .arg(&dir_store)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn follower");
    let mut follower_in = follower.stdin.take().expect("follower stdin");
    let mut follower_out = BufReader::new(follower.stdout.take().expect("follower stdout"));
    let mut line = String::new();
    follower_out.read_line(&mut line).expect("follower first line");
    println!("[t1] follower at open: {}", line.trim());
    assert_eq!(line.trim(), format!("OK {NODES}"));

    conn.query_with_params(CREATE_VERSION, version_params(NODES))
        .expect("writer commits beside readers");

    // Poll the follower until it reports the new commit, for at most 5 s.
    let start = Instant::now();
    let mut follower_saw_commit = false;
    while start.elapsed() < Duration::from_secs(5) {
        writeln!(follower_in, "poll").expect("poll follower");
        line.clear();
        follower_out.read_line(&mut line).expect("follower line");
        if line.trim() == format!("OK {}", NODES + 1) {
            follower_saw_commit = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    println!(
        "[t1] follower saw the writer's later commit: {follower_saw_commit} (after {:?}, last line: {})",
        start.elapsed(),
        line.trim()
    );
    drop(follower_in);
    follower.wait().expect("follower exit");

    // A fresh read-only open after the commit sees it too.
    let late_reader = probe("read-only", &dir_store);
    println!("[t1] fresh read-only open after the later commit: {late_reader}");
    assert_eq!(late_reader, format!("OK {}", NODES + 1));
    assert!(follower_saw_commit, "follower should see the writer's commit");
    println!("[t1] PASS shared reads: read-only and follow opens work beside the writer");

    drop(conn);
    drop(db);
    println!(
        "[t1] directory store size after close: {} bytes",
        size_on_disk(&dir_store)
    );
    let start = Instant::now();
    let db = Database::open(dir_uri).expect("reopen directory store");
    println!("[t1] reopen directory store: {:?}", start.elapsed());
    assert_eq!(count(&mut db.connect().expect("connect")), NODES + 1);
}
