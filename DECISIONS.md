# Design decisions

Decisions taken at the end of Phase 0, on 27 November 2026. Each one names
the Phase 0 evidence behind it; the measurements are in
[`phase0/RESULTS.md`](phase0/RESULTS.md). "Not yet tested" marks behaviour
that Phase 0 did not exercise and the build must cover with its own tests.

## 1. Storage: a Relate directory store

The graph lives in a Relate directory store, not a single `.rl` file.

- **Why.** A `.rl` file is locked whole by its writer, so a second process can
  neither write nor read it. A directory store still has one writer, but other
  processes can open it read-only or follow its commits. The CLI and an agent
  server must be able to run at the same time.
- **Evidence.** Test 1.

## 2. Schema: a `HAS_ROOT` edge for workspace members

Every snapshot has a `(Snapshot)-[:HAS_ROOT]->(Version)` edge to each
workspace member.

- **Why.** The draft schema could not answer "where does a dependency path
  start" from the graph alone. Explaining an invalidation needs a path from
  the project root to the affected version.
- **Evidence.** Test 4, where the explanation query starts from `HAS_ROOT`.

## 3. Approval digest: packages, edges and features

The digest that binds an approval covers three things:

1. the package list (name, version, source, checksum);
2. the resolved dependency edges;
3. the enabled features of each package.

- **Why.** A digest over the package list alone matched when the approved
  crate was added to a different workspace member. The package list was the
  same but the graph was not.
- **Evidence.** Test 5, extra observation.
- **Not yet tested.** Phase 0 implemented and tested the package-list digest
  only. Path dependencies stay normalised as
  `path:<manifest directory relative to the workspace root>`.

## 4. Invalidation: one per decision and advisory, by advisory kind

- An `Invalidation` is recorded once for each (decision, advisory) pair.
- Every `Advisory` has a kind: `vulnerability` or `notice`.
- A vulnerability invalidates an approval. A notice produces a warning by
  default and does not invalidate.

- **Why.** The Phase 0 query skipped a decision once it had any invalidation,
  so a second advisory against the same approval would go unrecorded. It also
  treated an "unmaintained" notice like a vulnerability.
- **Evidence.** Test 4 and its design notes.
- **Not yet tested.** The per-pair rule and the kind-based behaviour. Phase 0
  stored the RustSec `informational` value on the advisory and used it only to
  compare against `cargo audit`.

## 5. Primary graph: the `cargo metadata` resolve graph, all platforms

The resolve graph from `cargo metadata`, for all platforms, is the primary
dependency graph. Packages that appear only in `Cargo.lock` are stored as
well and flagged inactive.

- **Why.** The lockfile is a superset of the resolve graph: for `bat` it holds
  nine packages that no enabled feature uses. `cargo audit` reads the
  lockfile, so those packages must be visible, but they are not part of what
  is built.
- **Evidence.** Test 3 (optional measurements and the lockfile-superset test)
  and Test 2.
- **Not yet tested.** Storing and flagging inactive packages.

## 6. Simulation: two steps

A dependency change is simulated in a manifest-only temporary copy, in two
steps:

1. **Fast check.** Apply the change with `cargo add` or
   `cargo update --precise` and read the `Cargo.lock` it writes. Check that
   package list against policy.
2. **Full resolution.** Run `cargo metadata` only when the fast check finds a
   policy hit.

`--filter-platform` and `cargo generate-lockfile` are never used for
simulation.

- **Why.** The lockfile step downloads index data only and took 3.5 s on
  `bat` with a cold cache, against 20.6 s and 61 MB for `cargo metadata`. The
  lockfile contained every package of the resolve graph in all six Test 3
  cases, so the fast check does not miss a package there. `--filter-platform` drops packages used on other
  platforms, which would hide advisories against them.
  `cargo generate-lockfile` discards the existing lockfile and resolves every
  dependency again, so its result does not describe the real change.
- **Evidence.** Test 3: the gate cases, the optional measurements and the
  lockfile-superset test.
- **Not yet tested.** The two steps working together as one flow.

## 7. Advisory source: the OSV export

The RustSec OSV export is the only advisory source in the product.
`cargo audit` is ground truth in tests and nothing more.

- **Why.** The OSV export carries the `published`, `modified` and `withdrawn`
  dates that Living Approvals needs, and the product's own range evaluator
  agreed with `cargo audit` on every finding in both fixtures, so a second
  source is not needed at run time.
- **Evidence.** Test 4, Part A.

## 8. Unsupported inputs are detected and reported

The following are detected and reported as unsupported. They are never
resolved silently:

- git dependencies;
- `[patch]` sections;
- path dependencies outside the workspace root.

- **Why.** A manifest-only copy cannot see a path dependency outside the
  copied tree, and Phase 0 did not test the other two.
- **Evidence.** Test 3, "Not covered by these cases".
- **Not yet tested.** The detection itself.
