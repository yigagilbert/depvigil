# Phase 0 results

Feasibility tests for the dependency decision engine, as specified in
[`PHASE0_BRIEF.md`](../PHASE0_BRIEF.md). Every number below comes from a test
run on the machine described here. Timings are from the unoptimised `test`
profile.

## Environment

| Item | Value |
| --- | --- |
| Operating system | macOS 26.2 (25C56), arm64 |
| `rustc` | 1.88.0 (6b00bc388 2025-06-23) |
| `cargo` | 1.88.0 (873a06493 2025-05-10) |
| `relate` crate | 1.2.0, pinned as `=1.2.0` (see the note below) |
| `relate` CLI | 1.3.0, used only for inspecting stores by hand |
| `cargo-audit` | 0.23.0 |
| `small` fixture | this repository, commit `b83155e` |
| `public` fixture | `sharkdp/bat` v0.25.0, commit `25f4f96ea3afb6fe44552f3b38ed8b1540ffa1b3`, 208 locked packages |
| `advisories` fixture | `rustsec/advisory-db`, `osv` branch, commit `41c52cdab5fd303310c06be767cd48ad14970be9` (2026-11-27T10:36:00Z), 1,552 advisories, downloaded 2026-11-27 |

**Relate version.** The brief's API facts were checked against Relate 1.3.0,
but the `relate` crate 1.3.0 declares `rust-version = 1.99` and does not build
on Rust 1.88.0. Version 1.2.0 is the newest release that supports Rust 1.88,
so the tests run on 1.2.0. Re-running on 1.3.0 needs a toolchain upgrade to
Rust 1.99 or later.

**Fixture isolation.** `phase0/Cargo.toml` is its own workspace root and
excludes `fixtures/`. `cargo metadata` for `phase0` lists one workspace member
(`phase0`) and none of the fixture packages (`bat`, `small-app`, `small-core`,
`marker`, `time`). `regex` 1.13.2 and `smallvec` 1.16.6 appear only as
Relate's own dependencies, not at the fixture's pinned versions. No `target/`
directory and no build-script marker file exist under `fixtures/`.

## Test 1: Relate Rust crate basics

| Field | Content |
| --- | --- |
| Status | `PASS` |
| Evidence | `cargo test --test t1_relate -- --nocapture` in `phase0/`; test file `tests/t1_relate.rs`, second-process helper `src/bin/t1_probe.rs` |

**Pass criteria**

| Check | Result |
| --- | --- |
| Persistence | Pass. Count after reopen is exactly 1,000. |
| Rollback | Pass. Count is 1,001 inside the transaction and 1,000 after the rollback. |
| Read-only | Pass. The write fails with GQLSTATUS `42N18` (`WriteOnReadOnlyAccessDatabase`). |
| Single writer | Pass. A second writable open from another process fails with `Error::Locked` (`storage locked: <path>`, GQLSTATUS `42000`). The first writer keeps working and the store reopens with the expected count. |
| Shared reads | Pass. A read-only process reads the directory store while the writer holds it. |
| Parameters | Pass. Every statement in the test and the helper is a constant; values travel only through `query_with_params`. |

**Numbers**

| Measurement | `.rl` file | Directory store |
| --- | --- | --- |
| Write 1,000 nodes in one transaction | 146.1 ms | 132.0 ms |
| Size on disk after close | 360,448 bytes | 27,909 bytes |
| Reopen | 18.5 ms | 13.8 ms |

A follower process opened with `OpenMode::Follow` saw a commit made after it
opened within 76.8 ms, which includes the test's 20 ms polling interval.

**Surprises**

- The `relate` crate 1.3.0 needs Rust 1.99, while the Rust tutorial says
  "Rust 1.88 or later" and shows output from 1.3.0. On Rust 1.88, `cargo add
  relate` writes `relate = "1.2.0"` but still locks most `relate-*` sub-crates
  at 1.3.0, and the build fails.
- Getting 1.2.0 to build took three steps: `resolver = "3"` in the workspace,
  a regenerated lockfile, and `cargo update --precise 1.2.0` for
  `relate-parser` and `relate-splitter`. `relate-model` and `relate-names`
  stay at 1.3.0 in the lockfile because the 1.2.0 crates require `^1.3.0` of
  them.
- `Database::open` takes `&str`, not a `Path`.
- A `.rl` file cannot be opened read-only while a writer holds it. The second
  process gets the same `storage locked` error as a second writer. This
  matches the docs.
- For a directory store, the path in the locked error is the lock file inside
  it, `<store>/.cypherlite.writer`, not the store path.
- The locked error has the generic GQLSTATUS `42000`, so code must match on
  the `Error::Locked` variant rather than on the status code.

**Design decision: use a directory store.**

- *Why.* The CLI and an agent server must be able to run at the same time. A
  `.rl` file is locked whole by its writer, so a second process can neither
  write nor read it. A directory store still allows only one writer, but other
  processes can open it read-only beside that writer, and a follower sees new
  commits as they land.
- *Writes from two processes.* Relate never allows two writing processes on
  one store. When the agent server is running it should own the writer lock,
  and the CLI should send its writes to the server and open the store
  read-only for queries. When no server is running, the CLI can take the
  writer lock itself for the length of one command.
- *How to open a directory store from Rust.* There is no separate
  constructor. `Database::open(path)` creates a directory store for any path
  that is not `:memory:` and does not end in `.rl` (or the legacy `.cl`).
- *What `OpenOptions` adds.* `Database::open_with(path, OpenOptions)` sets the
  open mode and storage settings. `OpenOptions::with_mode(mode)` is the
  default with only `mode` changed:

  | `OpenMode` | Behaviour |
  | --- | --- |
  | `Writable` | The default. Takes the writer lock, appends to the WAL and checkpoints. |
  | `ReadOnly` | Loads the store once and refuses writes. Never creates a store. Works beside a directory store's writer. Same as `Database::open_read_only`. |
  | `Follow` | Read-only, and keeps applying the writer's commits from the WAL. Directory stores only. |
  | `Ephemeral` | Loads the store into memory and releases the source. |

  `Database::clone(source, dest, CloneOptions)` converts between a `.rl` file
  and a directory store, so a directory store can still be exported as one
  file.

  The test exercised `Writable`, `ReadOnly` and `Follow`. `Ephemeral` and
  `Database::clone` are described from the Relate docs and were not run.

## Test 3: isolated candidate resolution (gate)

| Field | Content |
| --- | --- |
| Status | `PASS` |
| Evidence | `cargo test --test t3_isolated -- --nocapture` in `phase0/`; test file `tests/t3_isolated.rs`, helpers in `tests/common/mod.rs` |

Each case runs four resolutions against one fresh, empty `CARGO_HOME`:
isolated with a cold cache, ground truth on a full copy, isolated again with
the warm cache, and isolated with `--offline`.

**Cases**

| # | Fixture | Change | Resolved packages | Edges | Equals ground truth |
| --- | --- | --- | --- | --- | --- |
| 1 | `small` | `cargo add getrandom@=0.2.15 -p small-app` | 17 | 19 | Yes |
| 2 | `small` | `cargo add smallvec@=1.6.1 -p small-app` (was `=1.6.0`) | 15 | 15 | Yes |
| 3 | `small` | `cargo add serde@=1.0.210 -p small-app --features derive` | 21 | 25 | Yes |
| 4 | `small` | `cargo add either@=1.13.0 -p small-core` | 16 | 16 | Yes |
| 5 | `public` | `cargo add petgraph@=0.6.5` | 201 | 357 | Yes |
| extra | `small` | `cargo update itoa --precise 1.0.10` | 15 | 15 | Yes |

The extra case is not one of the brief's five. It exercises the
`cargo update --precise` path that the method also allows.

**Pass criteria**

| Check | Result |
| --- | --- |
| Correctness | Pass, 5 of 5. Package ids, dependency edges (with kind and target), enabled features and workspace members all equal the ground truth. The resulting `Cargo.lock` is also byte-identical in every case. |
| Original untouched | Pass. After every case `git status --porcelain --ignored` on the fixture is empty and the `Cargo.lock` SHA-256 is unchanged (`small`: `1edfdfc3…731b195`, `public`: `a37c0c3c…535fcab`). |
| Nothing compiled | Pass. No `target/` directory exists in any temporary copy or in the fixtures. |
| No build script ran | Pass. `BUILD_SCRIPT_RAN.marker` exists nowhere. |
| Repeatable | Pass. The second isolated run gives the same resolve graph and the same `Cargo.lock`. |
| Offline | Pass. With the cache warmed by the first run, `cargo add --offline` and `cargo metadata --offline` give the ground-truth graph in every case. |

**Numbers**

| # | Isolated, cold cache | Isolated, warm cache | Isolated, `--offline` | Ground truth | Crates downloaded (cold) | Download size |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | 6.05 s | 2.33 s | 0.12 s | 1.18 s | 14 | 8,815,692 bytes |
| 2 | 3.30 s | 1.21 s | 0.11 s | 1.08 s | 12 | 8,750,223 bytes |
| 3 | 3.86 s | 1.36 s | 0.13 s | 1.25 s | 18 | 9,331,834 bytes |
| 4 | 4.53 s | 1.12 s | 0.12 s | 1.05 s | 13 | 8,769,221 bytes |
| 5 | 17.10 s | 1.74 s | 0.26 s | 5.54 s | 200 | 61,420,029 bytes |
| extra | 2.70 s | 0.73 s | 0.15 s | 0.81 s | 12 | 8,744,572 bytes |

Wall time covers the manifest copy, the stubs, the change and
`cargo metadata`. The ground-truth run always used the cache the cold run had
filled, and downloaded nothing further.

**What Cargo downloads**

- On a cold cache the simulation uses the network: Cargo queries the
  crates.io index and downloads the `.crate` archive of every package in the
  resolved graph, for all platforms. Case 1 fetched 14 archives:
  `aho-corasick`, `cfg-if`, `getrandom`, `itoa`, `libc`, `memchr`, `regex`,
  `regex-syntax`, `smallvec`, `time`, `wasi`, `winapi`,
  `winapi-i686-pc-windows-gnu` and `winapi-x86_64-pc-windows-gnu`.
- Archives are downloaded and unpacked into `CARGO_HOME` so Cargo can read
  their manifests. Nothing in them is compiled or run.
- On a warm cache without `--offline`, Cargo still contacts the crates.io
  index (`Updating crates.io index`) but downloads no archives.
- With `--offline` and a warm cache there is no network use, and the result
  is the same.

**Surprises**

- Cargo rejects a bare manifest copy: `no targets specified in the manifest`.
  Stubs are required in every case.
- `cargo metadata` downloads crates for platforms other than the host. The
  three Windows-only `winapi` crates account for most of the 8.7 MB fetched
  for the `small` fixture on macOS.
- `sharkdp/bat` contains a `Cargo.toml` that is syntax-highlighting test data
  (`tests/syntax-tests/source/TOML/Cargo.toml`). Copying "every `Cargo.toml`"
  picks it up and stubs it. It is outside the workspace, so the result is
  unaffected, but the product should copy only the manifests Cargo itself
  reports.
- The `resolve` graph of `bat` has 201 packages after the change, fewer than
  the 208 in its unchanged `Cargo.lock`, even though the change adds
  packages. The likely cause is that the lockfile also covers optional
  dependencies that no enabled feature uses; Test 2 should confirm this.
  Tests that compare against `cargo audit`, which reads `Cargo.lock`, must
  allow for the difference.
- In a workspace with a virtual root manifest, `cargo add` needs `-p <member>`.

**Design answers**

- *Are manifest stubs needed?* Yes. For every package manifest the helper
  creates an empty file at each declared target path and at `src/lib.rs` when
  no library path is declared. For `small` that is four empty files
  (`app/src/lib.rs`, `core/src/lib.rs`, `vendor/marker/src/lib.rs`,
  `vendor/marker/build.rs`). The helper also stubs a declared build script;
  whether Cargo needs that particular stub was not tested separately. A stub
  changes a package's target list but not the resolve graph.
- *What does Cargo download, and does offline mode work?* See above: index
  data and every resolved package's archive on a cold cache, index data only
  on a warm cache, nothing with `--offline`. Offline mode works once the cache
  holds the packages the candidate needs.

**Not covered by these cases**

Path dependencies outside the workspace root, git dependencies, `[patch]`
sections, alternative registries and vendored sources were not tested. A
manifest-only copy would miss a path dependency that lives outside the
copied tree.

## Authorship log

Kept for the AI-use disclosure the hackathon rules require.

| Part | Written by |
| --- | --- |
| `PHASE0_BRIEF.md` | Project author |
| `phase0/` crate, tests, helper binary and `small` fixture | Coding agent (Claude Code), directed and reviewed by the project author |
| `RESULTS.md` | Coding agent, from test output |
| `fixtures/public`, `fixtures/advisories/crates` | Third-party data, unmodified |
