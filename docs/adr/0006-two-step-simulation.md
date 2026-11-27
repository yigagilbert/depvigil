# 0006. Simulate a change in two steps

- **Status:** Accepted
- **Date:** 2026-11-27

## Context

Phase 0 measured a lockfile-only resolution at 3.5 s on `bat` with a cold cache and index data only, against 20.6 s and 61 MB of downloads for `cargo metadata`. In all six Test 3 cases the lockfile written by the change contained every package of the resolve graph.

## Decision

A dependency change is simulated in a manifest-only temporary copy:

1. **Fast check.** Apply the change with `cargo add` or `cargo update --precise` and read the `Cargo.lock` it writes. Check that package list against policy.
2. **Full resolution.** Run `cargo metadata` only when the fast check finds a policy hit.

`--filter-platform` and `cargo generate-lockfile` are never used for simulation. Nothing is ever built.

## Consequences

- A clean candidate is approved without downloading any crate archive.
- A candidate approved by the fast step has lockfile edges only.
- `--filter-platform` would hide advisories against packages used on other platforms. `cargo generate-lockfile` discards the existing lockfile, so its result does not describe the real change.
- This milestone simulates `cargo add` only. `cargo update --precise` was shown to work in Phase 0 but is not offered by `check` yet.

## Evidence

`tests/check.rs`: a clean candidate is approved with `cargo add` as the only Cargo command; a policy hit runs `cargo metadata`. `tests/ecosystem.rs`: the isolated result equals a full checkout, and the lockfile contains the resolve set.
