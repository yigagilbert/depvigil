# Phase 0 results

Feasibility tests for the dependency decision engine, as specified in
[`PHASE0_BRIEF.md`](../PHASE0_BRIEF.md). Every number below comes from a test
run on the machine described here. Timings are from the unoptimised `test`
profile.

## Environment

| Item | Value |
| --- | --- |
| Operating system | macOS 26.2 (25C56), arm64 |
| `rustc` | 1.100.0 (0ef1bc386 2026-11-09), pinned in `rust-toolchain.toml` |
| `cargo` | 1.100.0 (3acc79c0c 2026-11-05) |
| `relate` crate | 1.3.0, pinned as `=1.3.0`, no manual sub-crate pins |
| `relate` CLI | 1.3.0, used only for inspecting stores by hand |
| `cargo-audit` | 0.23.0 |
| `small` fixture | this repository, commit `b83155e` |
| `public` fixture | `sharkdp/bat` v0.25.0, commit `25f4f96ea3afb6fe44552f3b38ed8b1540ffa1b3`, 208 locked packages |
| `advisories` fixture | `rustsec/advisory-db`, `osv` branch, commit `41c52cdab5fd303310c06be767cd48ad14970be9` (2026-11-27T10:36:00Z), 1,552 advisories, downloaded 2026-11-27 |
| `advisory-db` fixture | `rustsec/advisory-db`, `main` branch, commit `c5279d0b0c9e8b70df666eb6d1edbabcfd1176bd` (2026-11-27T10:35:42Z), pinned submodule, used by `cargo audit` as ground truth |

Tests 1 and 3 were first run on Rust 1.88.0 with `relate` 1.2.0 and then
re-run on the toolchain above. The earlier numbers are kept as notes under
each test.

**Fixture isolation.** `phase0/Cargo.toml` is its own workspace root and
excludes `fixtures/`. `cargo metadata` for `phase0` lists one workspace member
(`phase0`) and none of the fixture packages (`bat`, `small-app`, `small-core`,
`marker`, `time`). `regex` and `smallvec` appear only as Relate's own
dependencies, not at the fixture's pinned versions. No `target/` directory
and no build-script marker file exist under `fixtures/`.

### Relate's minimum Rust version: evidence for a docs report

The Relate docs state a minimum of Rust 1.88:

- Rust tutorial (`/docs/get-started/rust/`), Prerequisites: "Rust 1.88 or
  later, with cargo." The same page shows output from release 1.3.0.
- "Use the Rust crate" guide: "Rust 1.88 or later and Cargo. The crate's
  `rust-version` is 1.88."

The published crate disagrees. `cargo info relate` reports
`version: 1.3.0` and `rust-version: 1.99`.

On Rust 1.88.0, `cargo add relate` in a new, empty crate printed:

```text
    Updating crates.io index
warning: ignoring relate@1.3.0 (which requires rustc 1.99) as it is incompatible with rustc 1.88.0
      Adding relate v1.2.0 to dependencies
             Features:
             + parquet
             - research
             - rkyv-codec
    Updating crates.io index
     Locking 160 packages to latest Rust 1.88.0 compatible versions
      Adding ordered-float v5.4.0 (available: v5.5.1, requires Rust 1.90)
      Adding relate v1.2.0 (available: v1.3.0, requires Rust 1.99)
      Adding relate-catalog v1.2.0 (available: v1.3.0, requires Rust 1.99)
      Adding relate-core v1.2.0 (available: v1.3.0, requires Rust 1.99)
      Adding relate-engine-api v1.2.0 (available: v1.3.0, requires Rust 1.99)
      Adding relate-error v1.2.0 (available: v1.3.0)
      Adding relate-executor v1.2.0 (available: v1.3.0, requires Rust 1.99)
      Adding relate-front v1.2.0 (available: v1.3.0, requires Rust 1.99)
      Adding relate-ir v1.2.0 (available: v1.3.0, requires Rust 1.99)
      Adding relate-markdown v1.2.0 (available: v1.3.0)
      Adding relate-ontology v1.2.0 (available: v1.3.0)
      Adding relate-planner v1.2.0 (available: v1.3.0, requires Rust 1.99)
      Adding relate-storage v1.2.0 (available: v1.3.0, requires Rust 1.99)
      Adding relate-write-local v1.2.0 (available: v1.3.0, requires Rust 1.99)
```

A second problem follows. The 1.2.0 that Cargo falls back to does not build
from that lockfile either. Cargo locks `relate-parser` at 1.3.0 beside
`relate-front` 1.2.0, and `cargo check` fails:

```text
note: `relate_parser::Expr` defined here
   --> relate-parser-1.3.0/src/ast/expr.rs:16:1
21  |     PathConstruct(Vec<Expr>),
    |     ------------- not covered
error: could not compile `relate-front` (lib) due to 3 previous errors
```

On Rust 1.88, `relate` 1.2.0 built only after `relate-parser` and
`relate-splitter` were pinned to 1.2.0 by hand with `cargo update --precise`.
`relate-model` and `relate-names` could not be pinned back, because the 1.2.0
crates require `^1.3.0` of them. On Rust 1.100.0, `relate = "=1.3.0"` resolves
all 17 `relate*` crates to 1.3.0 and builds with no pins.

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

**Numbers** (`relate` 1.3.0, Rust 1.100.0)

| Measurement | `.rl` file | Directory store |
| --- | --- | --- |
| Write 1,000 nodes in one transaction | 141.0 ms | 129.2 ms |
| Size on disk after close | 360,448 bytes | 27,909 bytes |
| Reopen | 12.8 ms | 13.1 ms |

A follower process opened with `OpenMode::Follow` saw a commit made after it
opened within 66.4 ms, which includes the test's 20 ms polling interval.

*Earlier run on `relate` 1.2.0, Rust 1.88.0:* all checks passed with the same
behaviour. Write 146.1 ms (`.rl`) and 132.0 ms (directory); sizes identical at
360,448 and 27,909 bytes; reopen 18.5 ms and 13.8 ms; follower 76.8 ms.

**Surprises**

- The crate's real minimum Rust version is 1.99, not the documented 1.88. See
  the evidence section above.
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

## Test 2: two snapshots and a Cypher diff

| Field | Content |
| --- | --- |
| Status | `PASS` |
| Evidence | `cargo test --test t2_snapshots -- --nocapture` in `phase0/`; test file `tests/t2_snapshots.rs`, loader in `tests/common/graph.rs` |

Snapshot A is `bat` v0.25.0 as committed. Snapshot B is a second copy with
one requirement changed by hand, `git2` from `"0.19"` to `"0.20"`. Both were
resolved with `cargo metadata` in temporary copies and loaded into one Relate
directory store.

| Snapshot | Versions | `DEPENDS_ON` edges |
| --- | --- | --- |
| A, baseline | 199 | 361 |
| B, candidate | 220 | 428 |

**Pass criteria**

| Check | Result |
| --- | --- |
| Added, removed, changed | Pass. Cypher returns exactly the plain-Rust sets: 33 versions added, 12 removed, 8 packages changed. |
| Path | Pass. Every returned path starts at the workspace root, ends at the target, exists edge by edge in snapshot B's `cargo metadata` output, and uses only edges whose `snapshot` property is B. |
| Shared nodes | Pass. The store holds 232 `Version` nodes for 232 distinct versions across both snapshots. 187 of them are included by both snapshots, each as one node. |
| Speed | Pass. Every diff query returned in under 5 ms. |

The eight changed packages are `form_urlencoded`, `git2`, `idna`,
`libgit2-sys`, `percent-encoding`, `quote`, `smallvec` and `url`.

**Numbers**

| Measurement | Value |
| --- | --- |
| Load snapshot A (one transaction) | 123.1 ms |
| Load snapshot B (one transaction) | 133.8 ms |
| Store size with one snapshot, after a checkpoint | 35,822 bytes |
| Store size with two snapshots, after a checkpoint | 46,909 bytes |
| Added query | 4.68 ms |
| Removed query | 3.00 ms |
| Changed query | 4.70 ms |
| Path query, property map on the variable-length edge, `LIMIT 5` | 14.70 ms, 5 paths |
| Path query, `SHORTEST 1` with a quantified path pattern | 60.24 ms, 1 path |

The path question was "why is `displaydoc` 0.2.7 in the candidate?". The
shortest answer has six edges: `bat` → `git2` 0.20.4 → `url` 2.5.9 → `idna`
1.1.0 → `idna_adapter` 1.2.2 → `icu_properties` 2.4.0 → `displaydoc` 0.2.7.

**Surprises**

- The brief's two query sketches ran on Relate unchanged, including the
  property map on a variable-length relationship
  (`-[:DEPENDS_ON*1..25 {snapshot: $cand}]->`).
- Changing one requirement moved far more than one package: 33 versions
  added and 12 removed, because the new `git2` pulled a newer `url` and with
  it the `icu_*` crates, and raised `quote` and `smallvec`.
- `SHORTEST 1` was about four times slower than the plain variable-length
  match with `LIMIT 5`, though still far under the limit.
- `cargo metadata` gives no checksums. The loader takes them from
  `Cargo.lock`.
- Snapshot A has 199 resolved versions while the same `Cargo.lock` lists 208
  packages. This is the lockfile-superset effect described under Test 3.

**Design decision: keep the `snapshot` property on `DEPENDS_ON`.**

Filtering on the property inside a variable-length path is accepted by
Relate, returns only edges of the requested snapshot, and takes 15 ms on a
store with two snapshots of about 200 packages each. Versions stay shared
between snapshots, so a second snapshot added 11,087 bytes to a 35,822-byte
store. The fallback of a separate edge type or node per snapshot was not
built or measured, because the property approach met every criterion. One
edge per dependency kind (`normal`, `dev`, `build`) is stored, with the kind
in the `kind` property.

Not measured: how the path query behaves with many snapshots in one store,
or on a graph much larger than 220 packages.

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

**Numbers** (Cargo 1.100.0)

| # | Isolated, cold cache | Isolated, warm cache | Isolated, `--offline` | Ground truth | Crates downloaded (cold) | Download size |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | 3.39 s | 1.07 s | 0.09 s | 0.94 s | 14 | 8,815,692 bytes |
| 2 | 3.44 s | 0.93 s | 0.08 s | 0.98 s | 12 | 8,750,223 bytes |
| 3 | 3.28 s | 1.04 s | 0.10 s | 0.95 s | 18 | 9,331,834 bytes |
| 4 | 3.28 s | 0.90 s | 0.08 s | 0.91 s | 13 | 8,769,221 bytes |
| 5 | 13.45 s | 2.65 s | 0.26 s | 3.87 s | 200 | 61,420,029 bytes |
| extra | 2.89 s | 0.68 s | 0.09 s | 0.72 s | 12 | 8,744,572 bytes |

Wall time covers the manifest copy, the stubs, the change and
`cargo metadata`. The ground-truth run always used the cache the cold run had
filled, and downloaded nothing further.

*Earlier run on Cargo 1.88.0:* all checks passed, with the same package and
edge counts and the same download counts and sizes. Wall times, as cold /
warm / offline / ground truth: case 1 6.05 / 2.33 / 0.12 / 1.18 s; case 2
3.30 / 1.21 / 0.11 / 1.08 s; case 3 3.86 / 1.36 / 0.13 / 1.25 s; case 4
4.53 / 1.12 / 0.12 / 1.05 s; case 5 17.10 / 1.74 / 0.26 / 5.54 s; extra
2.70 / 0.73 / 0.15 / 0.81 s.

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
  three Windows-only `winapi` crates account for most of the 8.8 MB fetched
  for the `small` fixture on macOS.
- `sharkdp/bat` contains a `Cargo.toml` that is syntax-highlighting test data
  (`tests/syntax-tests/source/TOML/Cargo.toml`). Copying "every `Cargo.toml`"
  picks it up and stubs it. It is outside the workspace, so the result is
  unaffected, but the product should copy only the manifests Cargo itself
  reports.
- The `resolve` graph is smaller than the lockfile. For case 5 it has 201
  packages while the `Cargo.lock` written by the same change has 210. The
  nine extra lockfile packages are listed under the optional measurements
  below. Tests that compare against `cargo audit`, which reads `Cargo.lock`,
  must allow for the difference.
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

### Optional measurements: lockfile-only resolution and `--filter-platform`

Evidence: `cargo test --test t3_optional -- --nocapture`, test file
`tests/t3_optional.rs`. These are measurements, not pass criteria. Each
variant ran once in its own manifest-only copy with its own empty
`CARGO_HOME`, so every figure is for a cold cache. Host triple:
`aarch64-apple-darwin`.

| Variant | What runs after `cargo add` |
| --- | --- |
| `metadata` | `cargo metadata` (the Test 3 method) |
| `filter-platform` | `cargo metadata --filter-platform aarch64-apple-darwin` |
| `add-only` | Nothing. The `Cargo.lock` that `cargo add` wrote is parsed. |
| `generate-lockfile` | `cargo generate-lockfile`, then `Cargo.lock` is parsed |

**Case 1 (`small`, `cargo add getrandom@=0.2.15 -p small-app`)**

| Variant | Wall time | Packages | Archives downloaded | Archive bytes | Index cache bytes |
| --- | --- | --- | --- | --- | --- |
| `metadata` | 3.57 s | 17 | 14 | 8,815,692 | 973,258 |
| `filter-platform` | 2.95 s | 13 | 10 | 1,720,020 | 973,258 |
| `add-only` | 1.71 s | 17 | 0 | 0 | 973,258 |
| `generate-lockfile` | 2.28 s | 17 | 0 | 0 | 973,258 |

**Case 5 (`public`, `cargo add petgraph@=0.6.5`)**

| Variant | Wall time | Packages | Archives downloaded | Archive bytes | Index cache bytes |
| --- | --- | --- | --- | --- | --- |
| `metadata` | 20.62 s | 201 | 200 | 61,420,029 | 12,408,665 |
| `filter-platform` | 11.11 s | 153 | 152 | 17,367,283 | 12,408,665 |
| `add-only` | 3.52 s | 210 | 0 | 0 | 12,408,665 |
| `generate-lockfile` | 18.67 s | 245 | 0 | 0 | 13,807,228 |

The `metadata` variant also unpacked the archives into `CARGO_HOME`:
119,641,732 bytes for case 1 and 573,527,235 bytes for case 5.

**How the package sets differ from the `cargo metadata` result**

- *`add-only`.* `cargo add` already writes the new `Cargo.lock`, and it
  downloads no archives, only index data. Case 1: the same 17 packages.
  Case 5: the 201 metadata packages plus nine more, all at the versions the
  original lockfile pinned: `bit-set` 0.5.3, `bit-vec` 0.6.3, `execute`
  0.2.13, `execute-command-macro` 0.1.9, `execute-command-macro-impl` 0.1.10,
  `execute-command-tokens` 0.1.7, `fancy-regex` 0.11.0, `generic-array` 1.1.1
  and `typenum` 1.17.0. The lockfile is a superset of the resolve graph.
- *`generate-lockfile`.* It discards the existing lockfile and resolves every
  dependency again at the newest allowed version. Case 1 gives the same 17
  packages, but for case 5 the result has 245 packages, of which 195 are not
  in the metadata result, and 151 metadata packages are missing. It does not
  describe what the change would do to the real workspace, so it is the wrong
  command for a simulation.
- *`--filter-platform`.* It reduces downloads: 10 archives instead of 14 for
  case 1 (1.7 MB instead of 8.8 MB), and 152 instead of 200 for case 5
  (17.4 MB instead of 61.4 MB). The cost is that packages used only on other
  platforms leave the graph: 4 for case 1 (`wasi`, `winapi` and its two
  companion crates) and 48 for case 5 (mostly `windows-*` crates). An advisory
  against one of those packages would then go unseen.

**What this suggests for the build.** Reading the `Cargo.lock` that
`cargo add` or `cargo update --precise` writes gives the full package list
with versions, sources and checksums in a fraction of the time and with no
archive downloads. It has no feature information, and its dependency lists
are not split by kind or platform, so `cargo metadata` is still the source
for the resolve graph used in Test 3. A two-stage simulation looks possible:
lockfile first for the advisory check, `cargo metadata` only when the full
graph is needed. This was measured on two cases only and is a suggestion, not
a tested design.

## Test 4: advisory matching and Living Approvals

| Field | Content |
| --- | --- |
| Status | `PASS` |
| Evidence | `cargo test --test t4_advisories -- --nocapture` in `phase0/`; test file `tests/t4_advisories.rs` |

All data lives in one Relate directory store. Three snapshots of `small` are
loaded: the baseline as committed, a target candidate (`either` added) and a
control candidate (`either` added and `regex` raised to 1.5.5). The two
candidates differ in exactly one version: `regex` 1.5.4 against 1.5.5.

**Advisory data**

`cargo audit` reads the advisory database in its own format, so the ground
truth uses the `main` branch of `rustsec/advisory-db` at commit `c5279d0`,
committed 18 seconds before the OSV export at `41c52cd`. The two sets were
compared by advisory id:

| Source | Advisories |
| --- | --- |
| OSV snapshot (`osv` branch) | 1,552, of which 25 withdrawn and 540 informational |
| `advisory-db` checkout (`main` branch) | 1,551 |

One advisory differs: `RUSTSEC-2025-0000` (`matrix-sdk-base`) is in the OSV
export but has no file in the `main` checkout. It affects no package in
either fixture. Every other id is in both.

**Part A: advisory matching**

| Check | Result |
| --- | --- |
| Matching | Pass. For the `small` baseline snapshot the graph gives the same three (advisory, package, version) triples as `cargo audit`: `RUSTSEC-2020-0071` `time` 0.1.43, `RUSTSEC-2021-0003` `smallvec` 1.6.0 and `RUSTSEC-2022-0013` `regex` 1.5.4. |
| Informational notices, compared separately | Pass. Both report none for `small`. |

The `small` fixture has only 15 packages, so the same evaluator was also run
over whole lockfiles, without the graph, and compared with `cargo audit`:

| Lockfile | Vulnerabilities (ours / `cargo audit`) | Informational notices (ours / `cargo audit`) | Agreement |
| --- | --- | --- | --- |
| `small` | 3 / 3 | 0 / 0 | Identical sets |
| `public` (`bat`, 208 packages) | 6 / 6 | 7 / 7 | Identical sets |

**Part B: invalidation**

D is the publication date of `RUSTSEC-2022-0013`, 2022-03-08T12:00:00Z. Both
approvals are dated 2022-03-07T12:00:00Z.

| Check | Result |
| --- | --- |
| Target found | Pass. The query returns one row: `decision-target`, `RUSTSEC-2022-0013`, `pkg:cargo/regex@1.5.4`. Before the later advisories were loaded it returned nothing. |
| Control safe | Pass. `decision-control` is not returned. Its snapshot still holds `smallvec` 1.6.0 and `time` 0.1.43, whose advisories were published before the approval, so the date filter is exercised too. |
| History kept | Pass. The `Decision` node read back before and after is identical (same id, label and four properties), and exactly one `Invalidation` node points at it. |
| Idempotent | Pass. A second run returns no rows and the store still holds one `Invalidation` node. |
| Explained | Pass. The graph returns the path `small-app` 0.1.0 → `regex` 1.5.4, which exists in the target snapshot's `cargo metadata` output. |
| Optional: withdrawal | Pass. After the advisory is marked withdrawn, `decision-target` is reported as valid again. The `Invalidation` node and the `Decision` node are left as they were. |

**Numbers**

| Measurement | Value |
| --- | --- |
| Advisories loaded | 1,552 (429 published before D, then 1,123) |
| Advisory load time | 167.5 ms in total (46.1 ms and 121.4 ms) |
| Matching time, range evaluation plus `AFFECTS` edges | 6.6 ms in total, 3 edges |
| Invalidation query | 0.79 ms |
| Explanation path query | 0.78 ms |

Matching ran against a store that holds only three snapshots of `small`, so
the matching time says little about a large graph.

**Surprises**

- Relate accepted `datetime()` on OSV timestamps, `datetime(null)` for a
  missing `withdrawn` date, date arithmetic (`datetime($d) - duration('P1D')`)
  and comparison between stored datetimes.
- The draft schema cannot answer "which versions are the project root" from
  the graph alone. The loader adds a `(Snapshot)-[:HAS_ROOT]->(Version)` edge
  for each workspace member, and the explanation query starts there.
- The OSV export and the `main` branch differ by one advisory even though
  they were committed 18 seconds apart.
- In the RustSec OSV export every advisory has one affected package, one
  `SEMVER` range and only `introduced` and `fixed` events. The evaluator
  rejects anything else instead of guessing.

**Design answer: which code evaluates OSV version ranges.** Our own
evaluator, about 15 lines in `tests/t4_advisories.rs` (`in_range`), using the
`semver` crate for version ordering. It walks a range's events in version
order: an `introduced` at or below the version turns "affected" on and a
`fixed` at or below it turns it off. It agreed with `cargo audit` on every
finding in both lockfiles: 9 vulnerabilities and 7 notices across lockfiles
of 15 and 208 packages. That is a small sample, and none of the affected versions was a
pre-release, so pre-release ordering is untested.

**Design notes for the build**

- The brief's invalidation sketch skips a decision once any `Invalidation`
  points at it. A second advisory that later affects the same approval would
  then go unrecorded. Checking for an existing invalidation per decision and
  advisory pair would avoid that. This was not tested.
- The sketch does not look at the advisory type, so an informational notice
  such as "unmaintained" would invalidate an approval just as a vulnerability
  does. The loader stores the type in an `informational` property so a policy
  can decide.
- An advisory against a version that is in the lockfile but not in the
  resolve graph is seen by `cargo audit` and not by a graph built from
  `cargo metadata`. `small` has no such package, so this did not show here.

## Test 5: approval binding and drift detection

| Field | Content |
| --- | --- |
| Status | `PASS` |
| Evidence | `cargo test --test t5_binding -- --nocapture` in `phase0/`; test file `tests/t5_binding.rs`, digest in `tests/common/graph.rs` (`canonical_list`, `digest`) |

The approved change is `cargo add either@=1.13.0 -p small-core`. It is
simulated in a manifest-only copy as in Test 3, and its digest is stored on
the `Snapshot` and on the `Decision`. Each case then applies a change to a
full copy of `small`, recomputes the digest from `cargo metadata`, and
compares it with the digest read back from the `Decision`.

**Cases**

| # | Situation | Expected | Result |
| --- | --- | --- | --- |
| 1 | The approved change is applied exactly | Match | Match, `770aad93…0df23221` |
| 2 | `either` 1.12.0 is applied instead of 1.13.0 | Mismatch, package named | Mismatch. The diff reports `changed: either 1.13.0 -> 1.12.0`. |
| 3 | `hex` 0.4.3 is added as well | Mismatch, extra crate named | Mismatch. The diff reports `added: hex 0.4.3`. |
| 4 | The approved state in a different, deeper directory | Match | Match, the same digest |

**Pass criteria**

| Check | Result |
| --- | --- |
| Cases | Pass, 4 of 4. |
| Explanation | Pass. Both mismatches load the observed state as a snapshot and run the Test 2 diff queries against the approved snapshot, which name the differing package. |
| Stability | Pass. Ten recomputations from ten `cargo metadata` runs give one value. |

The digest computed in the manifest-only simulation equals the digest of the
full checkout in case 1, so an approval made on a simulation binds the real
workspace.

**Numbers** (`public` fixture, 199 resolved packages)

| Measurement | Value |
| --- | --- |
| Canonical list plus SHA-256 | 0.34 ms |
| `cargo metadata` plus parsing, warm cache | 247 ms |

**Surprise: the digest does not cover dependency edges.** As an extra
observation, the approved crate was added to the other workspace member
(`small-app` instead of `small-core`). The package list is identical, so the
digest matches, although the dependency edges differ: `small-app` → `either`
exists only in the applied state. The digest specified in the brief detects a
different set of packages, not a different shape of graph. The same would
hold for a change in enabled features.

**Design answer: how path dependencies are normalised in the digest.**
Each canonical line is `name version source checksum`. For a registry package
the source is Cargo's source string and the checksum comes from `Cargo.lock`.
For a path dependency the source is `path:` followed by the directory of its
manifest relative to the workspace root, for example `path:app` or
`path:vendor/marker`, and the checksum is empty. The relative directory comes
from the `manifest_path` field of `cargo metadata` after the absolute
workspace root is removed. Package ids are not used, because they embed an
absolute `file://` path. The test asserts that no absolute path, `file://`
URL or temporary directory name appears in the list.

**Design notes for the build**

- If an approval should also bind where a dependency is attached and which
  features are on, the canonical list needs the resolved edges and feature
  sets as well. Both are already in the `cargo metadata` output.
- The digest is computed from `cargo metadata`, so it covers the resolve
  graph and not lockfile-only packages. An approval therefore does not notice
  a change to a package that is locked but unused.
- A path dependency outside the workspace root has no relative path under
  this scheme. The loader stops with an error for one; none of the fixtures
  has one.

## Design answers

- [x] **Single `.rl` file or directory store, and why (Test 1).** Directory
  store. A `.rl` file is locked whole by its writer, so a second process can
  neither write nor read it. A directory store allows read-only and follow
  opens beside the one writer, which the CLI and an agent server need.
- [x] **How to open a directory store from Rust (Test 1).**
  `Database::open(path)` with any path that is not `:memory:` and does not
  end in `.rl`. `Database::open_read_only(path)` or
  `Database::open_with(path, OpenOptions::with_mode(OpenMode::Follow))` for
  the second process.
- [x] **`snapshot` property on edges, or the fallback (Test 2).** Keep the
  `snapshot` property on `DEPENDS_ON`. Path queries filtered on it are
  correct and take 15 ms on the `public` fixture. The fallback was not built.
- [x] **Whether manifest stubs are needed for isolated resolution (Test 3).**
  Yes, always. Cargo rejects a manifest with no targets. Empty stub files are
  enough and do not change the resolve graph.
- [x] **What Cargo downloads during a simulation, and whether offline mode
  works (Test 3).** On a cold cache, crates.io index data and the archive of
  every resolved package for all platforms: 8.8 MB for `small`, 61.4 MB for
  `bat`. On a warm cache, index data only. With `--offline` and a warm cache,
  nothing, and the result is identical. Nothing downloaded is compiled or run.
- [x] **Which code evaluates OSV version ranges, and its agreement with
  `cargo audit` (Test 4).** Our own evaluator on top of the `semver` crate.
  It matched `cargo audit` exactly on both lockfiles: 9 vulnerabilities and
  7 informational notices.
- [x] **How path dependencies are normalised in the digest (Test 5).** As
  `path:<manifest directory relative to the workspace root>` with an empty
  checksum, taken from `manifest_path`, never from the package id.

## Recommendation

| Test | Status |
| --- | --- |
| 1. Relate Rust crate basics | `PASS` |
| 2. Two snapshots and a Cypher diff | `PASS` |
| 3. Isolated candidate resolution (gate) | `PASS` |
| 4. Advisory matching and Living Approvals | `PASS` (Part A and Part B) |
| 5. Approval binding and drift detection | `PASS` |

Tests 1 to 5 pass, so the decision table gives: **start the full build with
the frozen P0 scope.** Approval binding stays in P0.

Points to settle when the build starts, none of which blocks it:

1. Relate 1.3.0 needs Rust 1.99 or later. The toolchain is pinned to 1.100.0
   in `rust-toolchain.toml`.
2. The draft schema needs a way to find a snapshot's workspace members. The
   tests add `(Snapshot)-[:HAS_ROOT]->(Version)`.
3. The digest in the brief binds the package list only. Decide whether an
   approval should also bind dependency edges and features.
4. The invalidation query in the brief stops recording once a decision has
   one invalidation, and treats informational notices like vulnerabilities.
5. The resolve graph from `cargo metadata` is smaller than `Cargo.lock`.
   Decide which of the two the product treats as "the dependency set";
   `cargo audit` uses the lockfile.
6. The product should copy the manifests Cargo reports, not every file named
   `Cargo.toml`, and must handle path dependencies outside the workspace root.

## Authorship log

Kept for the AI-use disclosure the hackathon rules require.

| Part | Written by |
| --- | --- |
| `PHASE0_BRIEF.md` | Project author |
| `phase0/` crate, tests, helper binary and `small` fixture | Coding agent (Claude Code), directed and reviewed by the project author |
| `RESULTS.md` | Coding agent, from test output |
| `fixtures/public`, `fixtures/advisory-db`, `fixtures/advisories/crates` | Third-party data, unmodified |
