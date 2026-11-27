//! Second-process probe for Test 1.
//!
//! The Test 1 integration test spawns this binary to open a store that the
//! test process already holds, so the single-writer and shared-read checks
//! run across a real process boundary. It prints exactly one line per
//! observation: `OK <count>` or `ERR <variant> <gql_status> <message>`.
//!
//! Usage: `t1_probe <mode> <store path>` where mode is one of
//! `write`, `read-only`, `follow`.

use std::io::{self, BufRead, Write};
use std::process::ExitCode;

use relate::{error_info, Database, Error, OpenMode, OpenOptions};

const COUNT_VERSIONS: &str = "MATCH (v:Version) RETURN count(v) AS n";

fn variant(e: &Error) -> &'static str {
    match e {
        Error::Locked(_) => "Locked",
        Error::Storage(_) => "Storage",
        Error::Io(_) => "Io",
        Error::Semantic(_) => "Semantic",
        _ => "Other",
    }
}

fn report(result: relate::Result<i64>) {
    match result {
        Ok(n) => println!("OK {n}"),
        Err(e) => println!("ERR {} {} {e}", variant(&e), error_info(&e).gql_status),
    }
    io::stdout().flush().expect("flush stdout");
}

fn count(db: &Database) -> relate::Result<i64> {
    let result = db.connect()?.query(COUNT_VERSIONS)?;
    Ok(result.rows[0][0].as_int().unwrap_or(-1))
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let (Some(mode), Some(path)) = (args.get(1), args.get(2)) else {
        eprintln!("usage: t1_probe <write|read-only|follow> <store path>");
        return ExitCode::from(2);
    };

    match mode.as_str() {
        "write" => report(Database::open(path).and_then(|db| count(&db))),
        "read-only" => report(Database::open_read_only(path).and_then(|db| count(&db))),
        "follow" => {
            // Open once, then report the count again each time the parent
            // sends a line, to show whether later commits become visible.
            match Database::open_with(path, OpenOptions::with_mode(OpenMode::Follow)) {
                Ok(db) => {
                    report(count(&db));
                    for _ in io::stdin().lock().lines() {
                        report(count(&db));
                    }
                }
                Err(e) => report(Err(e)),
            }
        }
        _ => {
            eprintln!("unknown mode: {mode}");
            return ExitCode::from(2);
        }
    }
    ExitCode::SUCCESS
}
