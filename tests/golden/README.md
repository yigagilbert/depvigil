# Recorded ground truth

These files hold what `cargo audit` reported for the fixture lockfiles, so
that tests can compare against it on a plain clone, without the `advisory-db`
submodule and without `cargo-audit` installed.

| Field | Value |
| --- | --- |
| Tool | `cargo-audit` 0.23.0 |
| Advisory database | `rustsec/advisory-db`, `main`, commit `c5279d0b0c9e8b70df666eb6d1edbabcfd1176bd` |
| Command | `cargo audit --db fixtures/advisory-db --no-fetch --json -f fixtures/<name>/Cargo.lock` |
| Recorded | 2026-11-27 |

Each file keeps the advisory id, package and version of every vulnerability
and of every advisory-backed warning, taken from the tool's JSON output. The
rest of that output (settings, paths, full advisory text) is left out.

When the submodule and the tool are both present, the tests also run
`cargo audit` live and check that it still agrees with these files.
