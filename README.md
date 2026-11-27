# depvigil

Dependency decisions that stay true.

depvigil is a command-line tool and Rust library for teams whose AI coding
agents add dependencies. It answers three questions:

1. **Before** a dependency is added: what would this change really pull in,
   and does it bring a known vulnerability?
2. **After** it is added: is the workspace exactly what was approved?
3. **Later**: is that approval still sound, now that more is known?

The third question is the unusual one. An approval in depvigil is not a
one-time gate. It is a record in a graph, linked to the exact dependency
state it approved and to the advisory data it was judged against. When a new
advisory comes to affect a version in that state, the approval is
invalidated and depvigil shows the dependency path that leads to the problem.

> **Status:** early development (Milestone 1, the core engine). The command
> line and the store format may change. Cargo is the only supported
> ecosystem.

## What it does

| Command | What it does |
| --- | --- |
| `depvigil init` | Creates the store and records the workspace as the baseline |
| `depvigil advisories load <path>` | Loads OSV advisories and links them to the versions they affect |
| `depvigil check <crate>[@version]...` | Simulates adding one to four candidates and records a decision for each |
| `depvigil verify` | Checks that the workspace is exactly what the latest approval covered |
| `depvigil recheck` | Re-examines past approvals against the advisories known now |
| `depvigil log` | Lists every decision with its verdict, date and current status |
| `depvigil why <crate>` | Shows why a crate is in the baseline, and the decisions that mention it |

Every command takes `--json` for machine-readable output and `--project
<dir>` to point at a workspace other than the current directory.

Exit codes: `0` success; `1` a negative result (a rejected candidate, a
mismatch, an invalidated approval); `2` a usage error; `3` an unsupported
workspace; `4` any other error.

## Install

depvigil is not published to crates.io yet. Build it from source:

```bash
git clone https://github.com/yigagilbert/depvigil
cd depvigil
cargo install --path .
```

You need rustup; the toolchain (Rust 1.100.0) is pinned in
`rust-toolchain.toml` and installed on first use. depvigil calls `cargo`, so
Cargo must be on your `PATH` when you run it.

## Try the demo

```bash
examples/demo.sh
```

The demo works on a temporary copy of a small fixture workspace and builds
nothing. It:

1. records the baseline;
2. loads only the advisories published before 8 March 2022;
3. checks `chrono 0.4.19`, which is **rejected** with the advisory id and the
   dependency path;
4. checks `either 1.13.0`, which is **approved** from the lockfile alone;
5. applies the approved change with `cargo add`;
6. verifies that the workspace matches the approval;
7. loads the remaining advisories;
8. rechecks: the approval is now **invalidated**, because an advisory against
   `regex 1.5.4`, already in the workspace, has arrived.

Steps 2 and 7 simulate the passage of time, and the demo says so in its
output. In real use you load the current advisories and `recheck` whenever
you load newer ones.

**Network access.** On an empty Cargo cache the demo downloaded 16 crate
archives (8,999,759 bytes) plus about 1 MB of crates.io index data, and Cargo
unpacked them to about 126 MB in `CARGO_HOME`. Twelve archives are the
fixture's own dependency graph, fetched by `init`; four belong to the
rejected `chrono` candidate. The approved candidate, `verify`, `recheck` and
the advisory loads fetched no archive. Nothing downloaded is compiled or run.

## How it works

### Simulation in two steps

`check` never touches your workspace. It copies only the manifests and the
lockfile into a temporary directory, with empty stubs in place of source
files, and works there:

1. **Fast check.** It runs `cargo add` in the copy and reads the `Cargo.lock`
   that Cargo writes. This needs crates.io index data only. If no advisory
   affects anything new, the candidate is approved here.
2. **Full resolution.** Only when step 1 finds a policy hit does it run
   `cargo metadata`, to learn whether the affected package is really part of
   the build and to find the dependency path to it.

depvigil only ever runs `cargo add`, `cargo update`, `cargo metadata` and
`cargo tree`. It never runs `cargo build`, `check`, `test` or `run` on a
candidate, so no third-party code and no build script executes.

### Policy

`depvigil.toml` at the workspace root selects the policy mode:

```toml
[policy]
mode = "introduced"   # the default; or "strict"
```

- `introduced`: a candidate is rejected only when it introduces a vulnerable
  version the baseline lacks. Vulnerabilities already in the baseline are
  shown as context, and one that the candidate removes is reported as an
  improvement.
- `strict`: a candidate is rejected when its dependency set contains any
  vulnerability.

In both modes an informational notice (for example "unmaintained") warns and
does not reject, and so does a vulnerability in a package that is locked but
not part of the build.

### The approval digest

An approval is bound to a SHA-256 digest of the candidate state: every
lockfile package with its checksum, every lockfile dependency edge, each
workspace member's declared dependencies (requirement, features,
`default-features`, `optional`, kind, target and rename), each member's
`[features]` table, and the workspace resolver. `verify` recomputes it from
your lockfile and manifests without running Cargo, and names each line that
differs.

### Living Approvals

Each link between an advisory and a version records when this store first
saw it. `recheck` invalidates an approval when such a link to a version in
the approved state was first seen after the decision. A decision is never
edited; an invalidation is a new record pointing at it. If the advisory is
later withdrawn, the decision is reported as restored and the history stays.

## Why Relate

depvigil stores everything in [Relate](https://relatedb.ai), an embedded
Cypher graph database. The questions it has to answer are graph questions:

- *What changed?* Two snapshots share their version nodes, so a diff is a
  short pattern match.
- *Why is this crate here?* A shortest path from a workspace member to the
  version.
- *Which approvals does this advisory touch?* One pattern from decision to
  snapshot to version to advisory.

Relate runs in process, so there is no server to operate, and its
transactions keep a decision and its links consistent. Every query in
depvigil is a constant with parameters; no Cypher text is ever assembled
from input.

### Storage mode

The store is a Relate **directory store** at `.depvigil/store` in your
workspace, not a single `.rl` file. A `.rl` file is locked whole by its
writer, so a second process could not even read it. A directory store still
has one writer at a time, but read-only commands (`log`, `why`, `verify`)
work while another process holds it.

A Relate store is binary, so depvigil also appends every decision and
invalidation to `.depvigil/decisions.jsonl`, a plain-text mirror that reads
well in a pull request. The store remains the source of truth.

depvigil checks its own dependencies this way: this repository's `.depvigil`
directory is committed, and a test fails if the dependency set changes
without a recorded decision.

## Known limits

- **Cargo only.** The engine is written against an `Ecosystem` trait, but
  Cargo is the one implementation.
- **Adding a dependency only.** `check` simulates `cargo add`, including a
  new version requirement for an existing dependency. It does not simulate
  removing a dependency or `cargo update`.
- **Unsupported workspaces are refused.** Git dependencies, `[patch]`
  sections and path dependencies outside the workspace root are detected and
  reported with exit code 3. They are never resolved silently.
- **Features are bound as declared.** The digest covers the features your
  manifests ask for, not the feature set Cargo finally resolves. For one
  lockfile and one set of declarations these are equivalent, but a different
  Cargo version could resolve features differently without changing the
  digest.
- **Fast approvals know less.** A candidate approved by the fast step has
  lockfile edges only and no record of which packages are part of the build.
  `recheck` therefore treats any later vulnerability in its lockfile as
  invalidating, including one in a package that may be inactive.
- **Advisories.** Only RustSec advisories for crates.io in OSV format are
  understood, with `SEMVER` ranges made of `introduced` and `fixed` events.
  Any other shape is an error, not a guess. Pre-release version ordering is
  untested.
- **One writer.** Two depvigil commands that write cannot run at the same
  time on one workspace; the second reports that the store is in use.
- **Re-record the baseline.** After you apply an approved change, run
  `depvigil init` again before the next `check`.
- **Network.** `init` and a rejected candidate download crate archives on a
  cold Cargo cache. `check --offline` works once the cache is warm.
- **Toolchain.** Relate 1.3.0 needs Rust 1.99 or later.

## Documentation

- [`docs/adr/`](docs/adr/README.md): one record per design decision.
- [`docs/feasibility/phase0.md`](docs/feasibility/phase0.md): the
  feasibility report the design rests on, with its measurements.
- [`CONTRIBUTING.md`](CONTRIBUTING.md) and [`SECURITY.md`](SECURITY.md).

## How this project was built

The code, tests and documentation in this repository were written by an AI
coding agent (Claude Code) under human direction. A person set the goals,
wrote the briefs, made the design decisions recorded in `docs/adr/`, and
reviewed and accepted the results at each step. The feasibility report
includes a short authorship log.

## Licence

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT licence ([LICENSE-MIT](LICENSE-MIT))

at your option.
