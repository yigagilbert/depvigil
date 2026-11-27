//! The end-to-end demo (acceptance item 7): `examples/demo.sh` runs the
//! whole sequence on the `small` fixture and must exit zero.

#![cfg(unix)]

mod common;

use std::process::Command;

use common::{cargo_home, repo_root};

#[test]
fn the_demo_script_runs_the_whole_sequence_and_exits_zero() {
    let _fixture = require_fixture!("small");
    let output = Command::new("bash")
        .arg(repo_root().join("examples/demo.sh"))
        .env("DEPVIGIL", env!("CARGO_BIN_EXE_depvigil"))
        .env("CARGO_HOME", cargo_home())
        .env_remove("DEPVIGIL_NOW")
        .output()
        .expect("run the demo");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "the demo failed:\n{stdout}\n{stderr}"
    );

    // Each step of the sequence, in order.
    let expected = [
        "Recorded baseline",
        "simulating the passage of time",
        "REJECTED chrono@=0.4.19",
        "RUSTSEC-2020-0159",
        "path: small-app@0.1.0 -> chrono@0.4.19",
        "APPROVED either@=1.13.0",
        "lockfile check only; cargo metadata was not needed",
        "MATCH: the workspace is exactly what decision D0002 approved",
        "Loaded 1552 advisories as load-0002",
        "INVALIDATED (1 new, 0 standing)",
        "D0002 (add either@=1.13.0 to small-core): RUSTSEC-2022-0013 affects regex@1.5.4",
        "path: small-app@0.1.0 -> regex@1.5.4",
        "approved  invalidated by RUSTSEC-2022-0013",
        "Demo complete",
    ];
    let mut position = 0;
    for line in expected {
        match stdout[position..].find(line) {
            Some(found) => position += found + line.len(),
            None => panic!("the demo output is missing, or has out of order: `{line}`\n{stdout}"),
        }
    }
}
