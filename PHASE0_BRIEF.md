# Phase 0 feasibility brief: dependency decision engine on Relate

Nov 27, 2026 · @MOJASOKO

## Purpose

Phase 0 answers one question: can the core of the project be built reliably on Relate and Cargo? It runs five small tests over two days, 27 and 28 November 2026. No product code is written until the results are in.

The project is a dependency decision engine for AI coding agents. It simulates a proposed dependency change, records an approval tied to the resulting dependency graph, and later invalidates that approval when new advisory evidence arrives ("Living Approvals").

- **Go rule:** Tests 1 to 4 must pass before the full build starts.
- **Test 3 is the gate.** If isolated resolution cannot be made to match real Cargo resolution, stop and report. The what-if simulation is the main differentiator, and there is no point building around a broken one.
- **Test 5 is allowed to fail.** If it does, approval binding moves to P1 and the build still starts.
- **Output:** one `RESULTS.md` file in the format given at the end of this brief.

## Ground rules for the coding agent

Relate was released on 11 November 2026, so you have no training knowledge of it. Read the documentation before writing any Relate code, and never guess an API name.

**Read first**

1. [Relate index for agents](https://relatedb.ai/llms.txt)
2. [Rust tutorial](https://relatedb.ai/docs/get-started/rust/)
3. [Transactions and concurrency](https://relatedb.ai/docs/concepts/transactions-and-concurrency/)
4. [Limitations](https://relatedb.ai/docs/reference/limitations/)
5. Install the Relate agent skill: `relate skill install --agent <your agent name>`

**API facts from those pages, checked on 27 November 2026 against relate 1.3.0**

| Need | What the docs show |
| --- | --- |
| Open a store | `Database::open(":memory:")` or `Database::open("./name.rl")`, then `db.connect()` |
| Run Cypher | `conn.query(text)` returns a `QueryResult` with `rows`, `header` and `stats` |
| Parameters | `conn.query_with_params(text, params)` with a `BTreeMap<String, Value>` |
| Transactions | `conn.begin()`, `conn.commit()`, `conn.rollback()`. No nesting. |
| Read-only | `Database::open_read_only(path)`. A write fails with GQLSTATUS `42N18`. |
| Errors | `relate::Error`, with `error_info(&e)` giving `gql_status` and `retryable` |
| Writers | One writing process per store. A second writable open fails as locked. |
| Toolchain | Rust 1.88 or later |

The tutorial does not show how to open a directory store from Rust. Finding that out is part of Test 1.

**Safety limits**

- Never run `cargo build`, `cargo check`, `cargo test` or `cargo run` against a candidate dependency set. Those compile third-party code and run build scripts.
- Allowed Cargo commands on candidates: `cargo add`, `cargo update`, `cargo generate-lockfile`, `cargo metadata`, `cargo tree`.
- Never change the fixture repositories. All candidate work happens in a temporary copy.
- Use the real `relate` crate. Do not mock it.

**Do not build in Phase 0**

No CLI polish, MCP server, policy engine, Python adapter, report or UI. Each test is one integration test that runs with `cargo test` and prints its measurements.

**When blocked**

If a test is stuck for more than two hours, stop and write down what failed and why. Do not fake a pass or quietly change a pass criterion.

## Setup

Everything lives in one new public repository, started inside the contest period. Phase 0 code goes in a `phase0/` crate so it can be kept as evidence or deleted later.

**Tools**

- Rust 1.88 or later, with `cargo`.
- The `relate` crate, added with `cargo add relate` and then pinned to an exact version in `Cargo.toml`.
- The `relate` CLI, for inspecting stores by hand.
- `cargo-audit`, used only as ground truth in Test 4.

**Layout**

```text
phase0/
  Cargo.toml
  tests/            one file per test: t1_relate.rs ... t5_binding.rs
  fixtures/
    small/          a tiny workspace written for these tests
    public/         one public Rust repo, pinned to a commit
    advisories/     RustSec advisories in OSV JSON, pinned
  RESULTS.md
```

**Fixtures**

| Fixture | Contents | Why |
| --- | --- | --- |
| `small` | Two-member workspace, about five direct dependencies, `Cargo.lock` committed. At least two dependencies pinned to versions with a known RustSec advisory. One local path dependency whose `build.rs` writes a marker file. | Small enough to check every result by hand |
| `public` | One public Rust repository with 100 to 300 resolved packages, pinned to a commit | Realistic size for timing |
| `advisories` | RustSec advisories for crates.io in OSV JSON, pinned to one download | Offline, repeatable advisory data with `published`, `modified` and `withdrawn` dates |

RustSec states that it exports its data to the OSV format and that the data is on osv.dev. Record in `RESULTS.md` where the snapshot came from and its date.

**Record at the start of `RESULTS.md`:** operating system, `rustc` version, `cargo` version, `relate` crate version, and the commit of each fixture.

## Draft graph schema

All five tests use this schema. It is a draft: Test 2 decides one open design point, noted under the tables.

**Nodes**

| Label | Key properties | Meaning |
| --- | --- | --- |
| `Project` | `name` | The repository or workspace |
| `Snapshot` | `id`, `kind` (`baseline` or `candidate`), `created_at`, `digest` | One resolved dependency state |
| `Package` | `purl` without version, for example `pkg:cargo/serde` | Package identity, ecosystem-neutral |
| `Version` | `purl` with version, `source`, `checksum` | One exact package version from one source |
| `Advisory` | `id`, `published`, `modified`, `withdrawn` | One OSV advisory |
| `Policy` | `id`, `rule` | One rule a decision was judged against |
| `Decision` | `id`, `verdict`, `decided_at`, `digest` | An approval or rejection, never edited later |
| `Invalidation` | `id`, `at`, `reason` | A later finding that a decision no longer holds |

**Relationships**

| Pattern | Meaning |
| --- | --- |
| `(Project)-[:HAS_SNAPSHOT]->(Snapshot)` | Snapshots belonging to a project |
| `(Snapshot)-[:INCLUDES]->(Version)` | Versions present in that resolved state |
| `(Version)-[:OF]->(Package)` | Which package a version belongs to |
| `(Version)-[:DEPENDS_ON {snapshot, kind}]->(Version)` | A resolved dependency edge within one snapshot |
| `(Advisory)-[:AFFECTS]->(Version)` | Advisory range matched this version |
| `(Decision)-[:COMPARES]->(Snapshot)` | The baseline the candidate was judged against |
| `(Decision)-[:APPROVES]->(Snapshot)` or `[:REJECTS]` | The candidate and the verdict |
| `(Decision)-[:UNDER]->(Policy)` | The rule applied |
| `(Decision)-[:EVIDENCED_BY]->(Advisory)` | Evidence behind a rejection |
| `(Invalidation)-[:INVALIDATES]->(Decision)` | The decision that no longer holds |
| `(Invalidation)-[:CAUSED_BY]->(Advisory)` | The new evidence that caused it |

**Two rules the schema enforces**

- Package identity includes the source. The same name and version from a registry and from a git URL are two different `Version` nodes.
- History is never rewritten. A decision is invalidated by adding an `Invalidation` node, and the `Decision` node stays as it was.

**Open design point for Test 2:** the same `Version` can depend on different versions in different snapshots, so `DEPENDS_ON` carries a `snapshot` property. If filtering on that property is slow or awkward in path queries, the fallback is a separate edge type or node per snapshot. Test 2 measures this and picks one.

## Test 1: Relate Rust crate basics

**Question:** does the Rust crate give us persistence, parameters, transactions and a workable way to share one store between a CLI and an agent server?

**Steps**

1. Open `./t1.rl`, begin a transaction, create 1,000 `Version` nodes using `query_with_params`, and commit.
2. Drop the database, reopen the file, and count the nodes.
3. Begin a transaction, create one node, roll back, and count again.
4. Open the file with `open_read_only` and attempt a write.
5. While one writable handle is open, attempt a second writable open of the same file from a second process.
6. Find out from the docs how to open a directory store from Rust, then repeat step 5 with a read-only second process on the directory store.

**Pass criteria**

| Check | Pass when |
| --- | --- |
| Persistence | Count after reopen is exactly 1,000 |
| Rollback | Count is unchanged after the rollback |
| Read-only | The write fails with GQLSTATUS `42N18` |
| Single writer | The second writable open fails with a locked error and nothing is corrupted |
| Shared reads | A read-only process can read the directory store while the writer holds it, or the test documents clearly that it cannot |
| Parameters | No Cypher text is built by string concatenation anywhere in the test |

**Record:** time to write the 1,000 nodes, file size, and time to reopen.

**Design decision this test must produce:** single `.rl` file or directory store. The docs say a `.rl` file is exclusive to its writer, while a directory store allows read-only opens beside the writer. If the CLI and an MCP server must run at the same time, that points to a directory store or to routing everything through one process. State which one the project should use and why.

## Test 2: two snapshots and a Cypher diff

**Question:** can one Relate store hold two resolved dependency states and compare them correctly and quickly?

**Steps**

1. Run `cargo metadata --format-version 1` on the `public` fixture and load the `resolve` graph as snapshot A (`baseline`).
2. Make a second copy of the fixture, change one dependency version by hand, run `cargo metadata` again, and load it as snapshot B (`candidate`) in the same store.
3. Compute the expected difference in plain Rust from the two JSON files: versions added, versions removed, and packages whose version changed.
4. Produce the same three sets with Cypher.
5. For one added version, return a dependency path from the workspace root to it within snapshot B.

**Query sketches**

These show the intent. Validate the syntax against Relate before relying on it.

```cypher
// versions in the candidate that are not in the baseline
MATCH (:Snapshot {id: $cand})-[:INCLUDES]->(v:Version)
WHERE NOT EXISTS { (:Snapshot {id: $base})-[:INCLUDES]->(v) }
RETURN v.purl ORDER BY v.purl
```

```cypher
// why is this version in the candidate?
MATCH p = (root:Version {purl: $root})
          -[:DEPENDS_ON*1..25 {snapshot: $cand}]->
          (v:Version {purl: $target})
RETURN p LIMIT 5
```

**Pass criteria**

| Check | Pass when |
| --- | --- |
| Added, removed, changed | Each Cypher result equals the plain-Rust set exactly |
| Path | The returned path exists edge by edge in snapshot B's `cargo metadata` output |
| Shared nodes | A version present in both snapshots is one node, not two |
| Speed | Each diff query returns in under 1 second on the `public` fixture |

The 1-second limit is a proposed target for an interactive agent tool, not a published figure. Report the real numbers either way.

**Record:** load time per snapshot, store size with one and with two snapshots, and each query's time.

**Design decision this test must produce:** keep the `snapshot` property on `DEPENDS_ON`, or switch to the fallback described in the schema section.

## Test 3: isolated candidate resolution

**Question:** can we learn exactly what a dependency change would resolve to, without touching the real workspace and without compiling or running third-party code?

This is the gate test. Spend the most care here.

**Method under test**

1. Copy only the manifests into a temporary directory: every `Cargo.toml`, the `Cargo.lock`, and any `.cargo/config.toml`.
2. If Cargo refuses a manifest because its source files are missing, add an empty `src/lib.rs` or `src/main.rs` stub for that member. Record whether this was needed.
3. Apply the proposed change in the copy with `cargo add` or `cargo update --precise`.
4. Run `cargo metadata --format-version 1` in the copy and read the `resolve` graph.

**Ground truth**

For each case, apply the same change to a full second checkout of the fixture and run `cargo metadata` there. The two `resolve` graphs must be identical once absolute paths are normalised.

**Cases**

| # | Change | Fixture |
| --- | --- | --- |
| 1 | Add a crate that is not in the tree | `small` |
| 2 | Upgrade an existing direct dependency to a newer version | `small` |
| 3 | Add a crate with a non-default feature enabled | `small` |
| 4 | Add a crate to one member of the two-member workspace | `small` |
| 5 | Add a crate that is not in the tree | `public` |

**Pass criteria**

| Check | Pass when |
| --- | --- |
| Correctness | 5 of 5 cases give the same package set and the same dependency edges as the ground truth |
| Original untouched | `git status` on the fixture is clean and the `Cargo.lock` hash is unchanged after every case |
| Nothing compiled | No `target/` directory exists in the temporary copy |
| No build script ran | The marker file from the path dependency's `build.rs` does not exist |
| Repeatable | Running a case twice gives the same result |
| Offline | With a warm Cargo cache, each case also passes with `--offline` |

**Record:** wall time per case, whether stubs were needed, whether the network was used on the first run, and anything Cargo downloaded. `cargo metadata` fetches dependency manifests, so expect downloads on a cold cache. Report what was fetched, since the README must describe this honestly.

**If this test fails:** stop. Write down which case failed, the exact difference between the two graphs, and whether any alternative method worked.

## Test 4: advisory matching and Living Approvals

**Question:** can the graph match advisories to versions correctly, and then find past approvals that a newly published advisory makes invalid?

**Part A: advisory matching**

1. Load the pinned OSV advisories as `Advisory` nodes with `published`, `modified` and `withdrawn`.
2. For each `Version` in the `small` baseline snapshot, evaluate the OSV version ranges and create `AFFECTS` edges.
3. Run `cargo audit --json` on the same `Cargo.lock` with the same advisory data as ground truth.

**Part B: invalidation**

1. Pick one advisory that affects the `small` fixture. Call its publication date D.
2. Load only advisories published before D. Record a `Decision` approving the snapshot, dated the day before D.
3. Record a second, control `Decision` approving a snapshot that the advisory does not affect.
4. Load the remaining advisories, as if the database had just been updated.
5. Run the invalidation query and create `Invalidation` nodes for what it finds.

**Query sketch**

```cypher
// approvals made before an advisory that now affects them
MATCH (d:Decision {verdict: 'approved'})-[:APPROVES]->(s:Snapshot)
      -[:INCLUDES]->(v:Version)<-[:AFFECTS]-(a:Advisory)
WHERE a.published > d.decided_at
  AND a.withdrawn IS NULL
  AND NOT EXISTS { (:Invalidation)-[:INVALIDATES]->(d) }
RETURN d.id, a.id, v.purl
```

**Pass criteria**

| Check | Pass when |
| --- | --- |
| Matching | The set of (advisory, package, version) equals the vulnerabilities `cargo audit` reports. Informational notices, such as unmaintained crates, are compared separately. |
| Target found | The affected decision is returned, with the advisory and the version that caused it |
| Control safe | The control decision is not returned |
| History kept | The original `Decision` node is byte-for-byte unchanged, and one `Invalidation` node points at it |
| Idempotent | Running the query a second time creates no further `Invalidation` nodes |
| Explained | A path from the project root to the affected version can be returned for the invalidated decision |

**Optional, 30 minutes at most:** mark one advisory as withdrawn and confirm that a decision invalidated only by it can be reported as valid again.

**Record:** advisory count loaded, load time, matching time, and invalidation query time.

## Test 5: approval binding and drift detection

**Question:** can an approval be tied to one exact resolved state, so we can tell later whether the agent applied what was approved?

**Method under test**

1. Build a canonical list for a snapshot: one line per resolved package with name, version, source and checksum, sorted.
2. Hash the list with SHA-256 and store the result as `digest` on both the `Snapshot` and the `Decision`.
3. To verify, recompute the digest from the repository's current `cargo metadata` output and compare.

Absolute file paths must not enter the digest. Path dependencies carry machine-specific paths in their package identifiers, so normalise them to a path relative to the workspace root.

**Cases**

| # | Situation | Expected |
| --- | --- | --- |
| 1 | The approved change is applied exactly | Digests match |
| 2 | A different version of one crate is applied | Mismatch, and the diff query from Test 2 names the differing package |
| 3 | An extra crate is added beyond the approval | Mismatch, and the extra crate is named |
| 4 | The same approved state is checked out in a different directory | Digests match |

**Pass criteria**

| Check | Pass when |
| --- | --- |
| Cases | 4 of 4 give the expected result |
| Explanation | Every mismatch lists the packages that differ, not only a failed hash |
| Stability | Recomputing the digest ten times gives one value |

**Record:** digest computation time on the `public` fixture.

**If this test fails:** approval binding moves to P1. Note what made the digest unstable.

## Report format and decision

Write one `RESULTS.md`. Keep it factual: every number in it must come from a test run, never from an estimate.

**Per test**

| Field | Content |
| --- | --- |
| Status | `PASS`, `FAIL` or `PARTIAL` |
| Evidence | The command that reproduces the result, and the test file name |
| Numbers | The measurements the test asked for |
| Surprises | Anything in Relate or Cargo that behaved differently from the docs |
| Design decision | The answer the test was asked to produce, if any |

**Design answers the report must contain**

- [ ] Single `.rl` file or directory store, and why (Test 1)
- [ ] How to open a directory store from Rust (Test 1)
- [ ] `snapshot` property on edges, or the fallback (Test 2)
- [ ] Whether manifest stubs are needed for isolated resolution (Test 3)
- [ ] What Cargo downloads during a simulation, and whether offline mode works (Test 3)
- [ ] Which code evaluates OSV version ranges, and its agreement with `cargo audit` (Test 4)
- [ ] How path dependencies are normalised in the digest (Test 5)

**What happens next**

| Outcome | Action |
| --- | --- |
| Tests 1 to 5 pass | Start the full build with the frozen P0 scope |
| Tests 1 to 4 pass, Test 5 fails | Start the build, with approval binding moved to P1 |
| Test 3 fails | Stop. Review the failure before any further work. |
| Test 4 Part A fails | Start the build, but use an existing library for range matching instead of our own |
| Test 4 Part B fails | Stop. Living Approvals is the signature feature and needs a redesign. |
| Test 1 or 2 fails | Stop. Report the Relate behaviour and check the issue tracker at github.com/relatedb/relate/issues |

**Also note for the hackathon rules:** the repository must be public, carry the GitHub topic `relate`, and its README must later describe significant AI tool use. Keep a short log of which parts the coding agent wrote, so that disclosure is easy to write.
